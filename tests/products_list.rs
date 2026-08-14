//! Integration test: api-foundation + diesel + PostgreSQL (testcontainers).
//!
//! Run with: cargo test --test products_list
//! Requires: Docker daemon running, libpq installed.
//!
//! Shows how little code a developer writes for a complete AIP-compliant List
//! method: only `ProductField` + the diesel translation. Everything else
//! (parsing, validation, token encoding, consistency checks) is api-foundation.

use diesel::prelude::*;
use diesel::pg::PgConnection;
use testcontainers_modules::{postgres::Postgres, testcontainers::runners::AsyncRunner};

use api_foundation::{
    filter::{Comparator, FilterableField, TypedExpression, TypedFilter, Value},
    list::ListQuery,
    order_by::{Direction, OrderBy, OrderableField},
    pagination::{CursorEntry, CursorValue, Page, PageToken},
};

// ── Diesel schema ─────────────────────────────────────────────────────────────

diesel::table! {
    products (id) {
        id -> Int8,
        name -> Text,
        price -> Double,
    }
}

// ── Model ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = products)]
struct Product {
    id: i64,
    name: String,
    price: f64,
}

// ── ProductField ──────────────────────────────────────────────────────────────
//
// This enum is the ONLY API-specific definition the developer writes.
// api-foundation does the rest: filter parsing, ordering, token management.

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProductField {
    Name,
    Price,
}

impl FilterableField for ProductField {
    fn from_field_name(name: &str) -> Option<Self> {
        match name {
            "name" => Some(Self::Name),
            "price" => Some(Self::Price),
            _ => None,
        }
    }

    fn allowed_comparators(&self) -> &[Comparator] {
        match self {
            Self::Name => &[Comparator::Equal, Comparator::Has],
            Self::Price => &[
                Comparator::Equal,
                Comparator::LessThan,
                Comparator::LessThanOrEqual,
                Comparator::GreaterThan,
                Comparator::GreaterThanOrEqual,
            ],
        }
    }
}

impl OrderableField for ProductField {
    fn from_field_name(name: &str) -> Option<Self> {
        match name {
            "name" => Some(Self::Name),
            "price" => Some(Self::Price),
            _ => None,
        }
    }
}

// ── Repository ────────────────────────────────────────────────────────────────
//
// Translates ListQuery<ProductField> → diesel query → Page<Product>.
// No filter parsing, no token encoding, no consistency checks — all handled
// by api-foundation before this function is even called.

type BoxedQuery<'a> = products::BoxedQuery<'a, diesel::pg::Pg>;

/// Base query: only the filter applied, no cursor / ordering / limit.
/// Reused for both COUNT(*) and the paginated SELECT to avoid duplicating
/// the filter logic.
fn base_query(filter: Option<&TypedFilter<ProductField>>) -> QueryResult<BoxedQuery<'_>> {
    let mut q = products::table.into_boxed();
    if let Some(f) = filter {
        q = apply_filter(q, &f.expression)?;
    }
    Ok(q)
}

fn list_products(
    conn: &mut PgConnection,
    query: ListQuery<ProductField>,
) -> QueryResult<Page<Product>> {
    // 1. COUNT(*) — base query with filter only, no cursor
    let total_size: i64 = base_query(query.filter.as_ref())?
        .count()
        .get_result(conn)?;

    // 2. Paginated query — base query + cursor + ordering + limit
    let mut q = base_query(query.filter.as_ref())?;

    if let Some(ref token) = query.cursor {
        q = apply_cursor(q, token, query.order_by.as_ref());
    }
    q = apply_ordering(q, query.order_by.as_ref());

    let limit = (query.page_size + 1) as i64;
    let mut rows: Vec<Product> = q.limit(limit).load(conn)?;

    let has_next = rows.len() as u32 > query.page_size;
    if has_next {
        rows.pop();
    }

    let next_page_token = has_next.then(|| {
        let last = rows.last().unwrap();
        PageToken::new(build_cursor(last, query.order_by.as_ref()), query.fingerprint()).encode()
    });

    Ok(Page {
        items: rows,
        next_page_token,
        total_size: Some(total_size as u32),
    })
}

fn apply_filter<'a>(
    q: BoxedQuery<'a>,
    expr: &'a TypedExpression<ProductField>,
) -> QueryResult<BoxedQuery<'a>> {
    match expr {
        // Chaining .filter() calls is implicit AND in diesel
        TypedExpression::And(l, r) => apply_filter(apply_filter(q, l)?, r),
        TypedExpression::Restriction(r) => Ok(match (&r.field, &r.comparator, &r.value) {
            (ProductField::Name, Comparator::Equal, Value::String(s)) => {
                q.filter(products::name.eq(s.clone()))
            }
            (ProductField::Price, Comparator::GreaterThan, Value::Number(n)) => {
                q.filter(products::price.gt(n))
            }
            (ProductField::Price, Comparator::GreaterThanOrEqual, Value::Number(n)) => {
                q.filter(products::price.ge(n))
            }
            (ProductField::Price, Comparator::LessThan, Value::Number(n)) => {
                q.filter(products::price.lt(n))
            }
            (ProductField::Price, Comparator::LessThanOrEqual, Value::Number(n)) => {
                q.filter(products::price.le(n))
            }
            _ => {
                return Err(diesel::result::Error::QueryBuilderError(
                    "unsupported filter combination".into(),
                ))
            }
        }),
        // OR/NOT require BoxableExpression — belongs in api-foundation-diesel (future crate)
        _ => Err(diesel::result::Error::QueryBuilderError(
            "OR/NOT not supported in this example".into(),
        )),
    }
}

fn apply_ordering<'a>(q: BoxedQuery<'a>, order_by: Option<&OrderBy<ProductField>>) -> BoxedQuery<'a> {
    // Tuples give multi-column ORDER BY with id as a stable tiebreaker
    match order_by.and_then(|o| o.clauses.first()) {
        None => q.order(products::id.asc()),
        Some(clause) => match (&clause.field, &clause.direction) {
            (ProductField::Name, Direction::Asc) => {
                q.order((products::name.asc(), products::id.asc()))
            }
            (ProductField::Name, Direction::Desc) => {
                q.order((products::name.desc(), products::id.asc()))
            }
            (ProductField::Price, Direction::Asc) => {
                q.order((products::price.asc(), products::id.asc()))
            }
            (ProductField::Price, Direction::Desc) => {
                q.order((products::price.desc(), products::id.asc()))
            }
        },
    }
}

fn build_cursor(product: &Product, order_by: Option<&OrderBy<ProductField>>) -> Vec<CursorEntry> {
    let mut cursor = Vec::new();
    if let Some(ob) = order_by {
        if let Some(clause) = ob.clauses.first() {
            cursor.push(match clause.field {
                ProductField::Name => CursorEntry {
                    field_name: "name".to_string(),
                    value: CursorValue::String(product.name.clone()),
                },
                ProductField::Price => CursorEntry {
                    field_name: "price".to_string(),
                    value: CursorValue::Float64(product.price),
                },
            });
        }
    }
    // id is always last — stable tiebreaker regardless of ordering
    cursor.push(CursorEntry {
        field_name: "id".to_string(),
        value: CursorValue::Int64(product.id),
    });
    cursor
}

fn apply_cursor<'a>(
    q: BoxedQuery<'a>,
    token: &PageToken,
    order_by: Option<&OrderBy<ProductField>>,
) -> BoxedQuery<'a> {
    let cursor = token.cursor();

    let cursor_id = cursor
        .iter()
        .find(|e| e.field_name == "id")
        .and_then(|e| {
            if let CursorValue::Int64(n) = e.value {
                Some(n)
            } else {
                None
            }
        })
        .unwrap_or(0);

    let primary = order_by.and_then(|ob| ob.clauses.first());

    match primary {
        None => q.filter(products::id.gt(cursor_id)),
        Some(clause) => match &clause.field {
            ProductField::Price => {
                let cursor_price = cursor
                    .iter()
                    .find(|e| e.field_name == "price")
                    .and_then(|e| {
                        if let CursorValue::Float64(f) = e.value {
                            Some(f)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(f64::MIN);
                // Keyset formula: (price, id) >/< (cursor_price, cursor_id)
                match clause.direction {
                    Direction::Asc => q.filter(
                        products::price
                            .gt(cursor_price)
                            .or(products::price.eq(cursor_price).and(products::id.gt(cursor_id))),
                    ),
                    Direction::Desc => q.filter(
                        products::price
                            .lt(cursor_price)
                            .or(products::price.eq(cursor_price).and(products::id.gt(cursor_id))),
                    ),
                }
            }
            ProductField::Name => {
                let cursor_name = cursor
                    .iter()
                    .find(|e| e.field_name == "name")
                    .and_then(|e| {
                        if let CursorValue::String(s) = &e.value {
                            Some(s.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();
                match clause.direction {
                    Direction::Asc => q.filter(
                        products::name
                            .gt(cursor_name.clone())
                            .or(products::name.eq(cursor_name).and(products::id.gt(cursor_id))),
                    ),
                    Direction::Desc => q.filter(
                        products::name
                            .lt(cursor_name.clone())
                            .or(products::name.eq(cursor_name).and(products::id.gt(cursor_id))),
                    ),
                }
            }
        },
    }
}

// ── Simulated tonic handler ───────────────────────────────────────────────────
//
// In a real app this is a method inside:
//
//   #[tonic::async_trait]
//   impl ProductsService for ProductsServiceImpl {
//       async fn list_products(
//           &self,
//           req: Request<ListProductsRequest>,
//       ) -> Result<Response<ListProductsResponse>, Status> {
//           let req = req.into_inner();
//           let query = ListQuery::<ProductField>::build(
//               Some(&req.filter),
//               Some(&req.order_by),
//               req.page_size,
//               Some(&req.page_token),
//           )?;
//           let page = self.repo.list(query).await?;
//           Ok(Response::new(ListProductsResponse { ... }))
//       }
//   }
//
// The handler stays this simple no matter how complex the filtering/ordering is.

fn handle_list_products(
    conn: &mut PgConnection,
    filter: &str,
    order_by: &str,
    page_size: i32,
    page_token: &str,
) -> Result<Page<Product>, api_foundation::error::Error> {
    let query = ListQuery::<ProductField>::build(
        Some(filter),
        Some(order_by),
        page_size,
        Some(page_token),
    )?;
    list_products(conn, query)
        .map_err(|e| api_foundation::error::Error::InvalidFilter(e.to_string()))
}

// ── Test helpers ──────────────────────────────────────────────────────────────

fn setup(conn: &mut PgConnection) {
    diesel::sql_query(
        "CREATE TABLE IF NOT EXISTS products (
            id    BIGSERIAL PRIMARY KEY,
            name  TEXT NOT NULL,
            price DOUBLE PRECISION NOT NULL
        )",
    )
    .execute(conn)
    .unwrap();

    diesel::sql_query("DELETE FROM products")
        .execute(conn)
        .unwrap();

    diesel::sql_query(
        "INSERT INTO products (name, price) VALUES
            ('Alpha',   10.0),
            ('Beta',    20.0),
            ('Gamma',   30.0),
            ('Delta',   40.0),
            ('Epsilon', 50.0)",
    )
    .execute(conn)
    .unwrap();
}

async fn pg_conn() -> (PgConnection, impl Drop) {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres");
    let conn = PgConnection::establish(&url).unwrap();
    (conn, container)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pagination_by_id() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    // Page 1 — page_size = 2
    let p1 = handle_list_products(&mut conn, "", "", 2, "").unwrap();
    assert_eq!(p1.items.len(), 2);
    assert!(p1.next_page_token.is_some());

    // Page 2
    let p2 = handle_list_products(&mut conn, "", "", 2, p1.next_page_token.as_deref().unwrap()).unwrap();
    assert_eq!(p2.items.len(), 2);
    assert!(p2.next_page_token.is_some());

    // Page 3 — last page
    let p3 = handle_list_products(&mut conn, "", "", 2, p2.next_page_token.as_deref().unwrap()).unwrap();
    assert_eq!(p3.items.len(), 1);
    assert!(p3.next_page_token.is_none());

    // total_size is stable across pages — always reflects the full filtered collection
    assert_eq!(p1.total_size, Some(5));
    assert_eq!(p2.total_size, Some(5));
    assert_eq!(p3.total_size, Some(5));

    // All 5 products are returned, each exactly once
    let all_ids: Vec<i64> = [p1.items, p2.items, p3.items]
        .concat()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(all_ids.len(), 5);
    let unique: std::collections::HashSet<_> = all_ids.iter().collect();
    assert_eq!(unique.len(), 5);
}

#[tokio::test]
async fn filter_by_price() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    let page = handle_list_products(&mut conn, "price > 25", "", 10, "").unwrap();
    assert_eq!(page.items.len(), 3); // 30, 40, 50
    assert!(page.items.iter().all(|p| p.price > 25.0));
    assert!(page.next_page_token.is_none());
    assert_eq!(page.total_size, Some(3));
}

#[tokio::test]
async fn filter_combined_with_pagination() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    // Only products with price > 10 (Beta, Gamma, Delta, Epsilon = 4 items), page 2 of 2
    let p1 = handle_list_products(&mut conn, "price > 10", "", 2, "").unwrap();
    assert_eq!(p1.items.len(), 2);
    assert_eq!(p1.total_size, Some(4));
    assert!(p1.next_page_token.is_some());

    let p2 = handle_list_products(&mut conn, "price > 10", "", 2, p1.next_page_token.as_deref().unwrap()).unwrap();
    assert_eq!(p2.items.len(), 2);
    assert_eq!(p2.total_size, Some(4)); // same total regardless of which page
    assert!(p2.next_page_token.is_none());
}

#[tokio::test]
async fn pagination_with_ordering_desc() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    let p1 = handle_list_products(&mut conn, "", "price desc", 2, "").unwrap();
    assert_eq!(p1.items[0].price, 50.0); // highest first
    assert_eq!(p1.items[1].price, 40.0);

    let p2 = handle_list_products(&mut conn, "", "price desc", 2, p1.next_page_token.as_deref().unwrap()).unwrap();
    assert_eq!(p2.items[0].price, 30.0);
    assert_eq!(p2.items[1].price, 20.0);

    let p3 = handle_list_products(&mut conn, "", "price desc", 2, p2.next_page_token.as_deref().unwrap()).unwrap();
    assert_eq!(p3.items[0].price, 10.0);
    assert!(p3.next_page_token.is_none());
}

#[tokio::test]
async fn token_mismatch_is_rejected() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    // Get a valid token for filter="price > 10", order_by="price asc"
    let p1 = handle_list_products(&mut conn, "price > 10", "price asc", 2, "").unwrap();
    let token = p1.next_page_token.as_deref().unwrap();

    // Different filter — rejected
    let err = handle_list_products(&mut conn, "price > 20", "price asc", 2, token).unwrap_err();
    assert!(matches!(err, api_foundation::error::Error::PageTokenMismatch));

    // Different order_by — rejected
    let err = handle_list_products(&mut conn, "price > 10", "price desc", 2, token).unwrap_err();
    assert!(matches!(err, api_foundation::error::Error::PageTokenMismatch));

    // Same parameters — accepted
    assert!(handle_list_products(&mut conn, "price > 10", "price asc", 2, token).is_ok());

    // Different page_size — accepted (AIP-158: page_size not part of fingerprint)
    assert!(handle_list_products(&mut conn, "price > 10", "price asc", 5, token).is_ok());
}

#[tokio::test]
async fn unknown_field_rejected_before_db() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    let err = handle_list_products(&mut conn, r#"secret = "x""#, "", 10, "").unwrap_err();
    assert!(matches!(
        err,
        api_foundation::error::Error::UnknownField { field } if field == "secret"
    ));
}

#[tokio::test]
async fn disallowed_comparator_rejected_before_db() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    // name only allows Equal and Has, not GreaterThan
    let err = handle_list_products(&mut conn, "name > 0", "", 10, "").unwrap_err();
    assert!(matches!(
        err,
        api_foundation::error::Error::DisallowedComparator { field, .. } if field == "name"
    ));
}
