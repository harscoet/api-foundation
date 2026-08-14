//! Integration test: api-foundation + diesel + PostgreSQL (testcontainers).
//!
//! Run with: cargo test --test products_list
//! Requires: Docker daemon running, libpq installed.
//!
//! Shows how little code a developer writes for a complete AIP-compliant List
//! method: only `ProductField` + `ProductAdapter` (column mapping). Everything
//! else (parsing, validation, AND recursion, token encoding, keyset pagination,
//! total_size) is provided by `DieselListQuery`.

use std::marker::PhantomData;

use diesel::prelude::*;
use diesel::pg::{Pg, PgConnection};
use testcontainers_modules::{postgres::Postgres, testcontainers::runners::AsyncRunner};

use api_foundation::{
    filter::{Comparator, FilterableField, TypedExpression, TypedRestriction, Value},
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
// The field enum is the ONLY domain-specific definition the developer writes.
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

// ── Generic framework — future `api-foundation-diesel` ────────────────────────
//
// `DieselList<F>` + `DieselListQuery<F, A>` would live in a dedicated crate.
// All entity-agnostic logic is here: COUNT(*) reuse, AND recursion, keyset
// cursor application, page_size+1 detection, token encoding.

/// Implement this trait once per entity to get `DieselListQuery::load_page`.
///
/// Only entity-specific column mappings belong here; all orchestration is
/// handled generically by `DieselListQuery`.
trait DieselList<F> {
    /// The diesel boxed query for this entity's table.
    /// Must be `'static` (achieved by cloning/copying all filter values).
    type BoxedQuery;
    /// The Queryable record returned by this table.
    type Record;

    fn empty_query() -> Self::BoxedQuery;

    /// Apply a single filter restriction. AND is handled generically in
    /// `DieselListQuery::apply_expression` — this only sees individual leaves.
    fn apply_restriction(
        q: Self::BoxedQuery,
        r: &TypedRestriction<F>,
    ) -> QueryResult<Self::BoxedQuery>;

    fn apply_ordering(q: Self::BoxedQuery, order_by: Option<&OrderBy<F>>) -> Self::BoxedQuery;

    fn apply_cursor(
        q: Self::BoxedQuery,
        token: &PageToken,
        order_by: Option<&OrderBy<F>>,
    ) -> Self::BoxedQuery;

    fn build_cursor_entries(
        record: &Self::Record,
        order_by: Option<&OrderBy<F>>,
    ) -> Vec<CursorEntry>;

    fn load(
        q: Self::BoxedQuery,
        conn: &mut PgConnection,
        limit: i64,
    ) -> QueryResult<Vec<Self::Record>>;

    fn count(q: Self::BoxedQuery, conn: &mut PgConnection) -> QueryResult<i64>;
}

/// Generic list query executor. Wraps a `ListQuery<F>` and an adapter `A`.
///
/// Provides `load_page` which:
/// 1. Calls `A::count` on the base query (filter only) → `total_size`
/// 2. Calls `A::load` on the full query (filter + cursor + ordering + limit)
/// 3. Builds the next page token if needed
struct DieselListQuery<'a, F, A: DieselList<F>> {
    list: &'a ListQuery<F>,
    _adapter: PhantomData<A>,
}

impl<'a, F: FilterableField + OrderableField, A: DieselList<F>> DieselListQuery<'a, F, A> {
    fn new(list: &'a ListQuery<F>) -> Self {
        Self { list, _adapter: PhantomData }
    }

    /// Base query: filter applied, no cursor / ordering / limit.
    /// Called twice — once for COUNT(*), once for the paginated SELECT.
    fn base(&self) -> QueryResult<A::BoxedQuery> {
        let mut q = A::empty_query();
        if let Some(ref f) = self.list.filter {
            q = self.apply_expression(q, &f.expression)?;
        }
        Ok(q)
    }

    /// Walk the typed expression tree. AND is handled here generically;
    /// individual restrictions are delegated to the adapter.
    fn apply_expression(
        &self,
        q: A::BoxedQuery,
        expr: &TypedExpression<F>,
    ) -> QueryResult<A::BoxedQuery> {
        match expr {
            TypedExpression::And(l, r) => {
                self.apply_expression(self.apply_expression(q, l)?, r)
            }
            TypedExpression::Restriction(r) => A::apply_restriction(q, r),
            _ => Err(diesel::result::Error::QueryBuilderError(
                "OR/NOT not yet supported — use api-foundation-diesel for full support".into(),
            )),
        }
    }

    fn total_size(&self, conn: &mut PgConnection) -> QueryResult<u32> {
        Ok(A::count(self.base()?, conn)? as u32)
    }

    fn load_page(&self, conn: &mut PgConnection) -> QueryResult<Page<A::Record>> {
        let total = self.total_size(conn)?;

        let mut q = self.base()?;
        if let Some(ref token) = self.list.cursor {
            q = A::apply_cursor(q, token, self.list.order_by.as_ref());
        }
        q = A::apply_ordering(q, self.list.order_by.as_ref());

        let limit = (self.list.page_size + 1) as i64;
        let mut rows = A::load(q, conn, limit)?;

        let has_next = rows.len() as u32 > self.list.page_size;
        if has_next {
            rows.pop();
        }

        let next_page_token = has_next.then(|| {
            let last = rows.last().unwrap();
            PageToken::new(
                A::build_cursor_entries(last, self.list.order_by.as_ref()),
                self.list.fingerprint(),
            )
            .encode()
        });

        Ok(Page { items: rows, next_page_token, total_size: Some(total) })
    }
}

// ── ProductAdapter ────────────────────────────────────────────────────────────
//
// This + ProductField is everything a developer writes for a new entity.
// The trait impl maps domain fields to diesel columns; the framework does the
// rest (count, AND recursion, token building, pagination).

struct ProductAdapter;

impl DieselList<ProductField> for ProductAdapter {
    // into_boxed() returns BoxedQuery<'static> when no borrowed data is stored.
    // All filter values below are cloned/copied, so the lifetime stays 'static.
    type BoxedQuery = products::BoxedQuery<'static, Pg>;
    type Record = Product;

    fn empty_query() -> Self::BoxedQuery {
        products::table.into_boxed()
    }

    fn apply_restriction(
        q: Self::BoxedQuery,
        r: &TypedRestriction<ProductField>,
    ) -> QueryResult<Self::BoxedQuery> {
        Ok(match (&r.field, &r.comparator, &r.value) {
            (ProductField::Name, Comparator::Equal, Value::String(s)) => {
                q.filter(products::name.eq(s.clone()))
            }
            (ProductField::Price, Comparator::Equal, Value::Number(n)) => {
                q.filter(products::price.eq(*n))
            }
            (ProductField::Price, Comparator::GreaterThan, Value::Number(n)) => {
                q.filter(products::price.gt(*n))
            }
            (ProductField::Price, Comparator::GreaterThanOrEqual, Value::Number(n)) => {
                q.filter(products::price.ge(*n))
            }
            (ProductField::Price, Comparator::LessThan, Value::Number(n)) => {
                q.filter(products::price.lt(*n))
            }
            (ProductField::Price, Comparator::LessThanOrEqual, Value::Number(n)) => {
                q.filter(products::price.le(*n))
            }
            _ => {
                return Err(diesel::result::Error::QueryBuilderError(
                    "unsupported filter combination".into(),
                ))
            }
        })
    }

    fn apply_ordering(
        q: Self::BoxedQuery,
        order_by: Option<&OrderBy<ProductField>>,
    ) -> Self::BoxedQuery {
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

    fn apply_cursor(
        q: Self::BoxedQuery,
        token: &PageToken,
        order_by: Option<&OrderBy<ProductField>>,
    ) -> Self::BoxedQuery {
        let cursor = token.cursor();

        let cursor_id = cursor
            .iter()
            .find(|e| e.field_name == "id")
            .and_then(|e| {
                if let CursorValue::Int64(n) = e.value { Some(n) } else { None }
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
                            if let CursorValue::Float64(f) = e.value { Some(f) } else { None }
                        })
                        .unwrap_or(f64::MIN);
                    // Keyset: (price, id) > (cursor_price, cursor_id)
                    match clause.direction {
                        Direction::Asc => q.filter(
                            products::price
                                .gt(cursor_price)
                                .or(products::price
                                    .eq(cursor_price)
                                    .and(products::id.gt(cursor_id))),
                        ),
                        Direction::Desc => q.filter(
                            products::price
                                .lt(cursor_price)
                                .or(products::price
                                    .eq(cursor_price)
                                    .and(products::id.gt(cursor_id))),
                        ),
                    }
                }
                ProductField::Name => {
                    let cursor_name = cursor
                        .iter()
                        .find(|e| e.field_name == "name")
                        .and_then(|e| {
                            if let CursorValue::String(s) = &e.value { Some(s.clone()) } else { None }
                        })
                        .unwrap_or_default();
                    match clause.direction {
                        Direction::Asc => q.filter(
                            products::name
                                .gt(cursor_name.clone())
                                .or(products::name
                                    .eq(cursor_name)
                                    .and(products::id.gt(cursor_id))),
                        ),
                        Direction::Desc => q.filter(
                            products::name
                                .lt(cursor_name.clone())
                                .or(products::name
                                    .eq(cursor_name)
                                    .and(products::id.gt(cursor_id))),
                        ),
                    }
                }
            },
        }
    }

    fn build_cursor_entries(
        product: &Product,
        order_by: Option<&OrderBy<ProductField>>,
    ) -> Vec<CursorEntry> {
        let mut cursor = Vec::new();
        if let Some(clause) = order_by.and_then(|ob| ob.clauses.first()) {
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
        // id is always last — stable tiebreaker regardless of ordering
        cursor.push(CursorEntry {
            field_name: "id".to_string(),
            value: CursorValue::Int64(product.id),
        });
        cursor
    }

    fn load(
        q: Self::BoxedQuery,
        conn: &mut PgConnection,
        limit: i64,
    ) -> QueryResult<Vec<Product>> {
        q.limit(limit).load(conn)
    }

    fn count(q: Self::BoxedQuery, conn: &mut PgConnection) -> QueryResult<i64> {
        q.count().get_result(conn)
    }
}

// ── Repository ────────────────────────────────────────────────────────────────
//
// The entire list implementation is one line. DieselListQuery handles:
// COUNT(*) reuse, AND recursion, cursor application, ordering, page_size+1
// detection, and next_page_token encoding.

fn list_products(
    conn: &mut PgConnection,
    query: ListQuery<ProductField>,
) -> QueryResult<Page<Product>> {
    DieselListQuery::<ProductField, ProductAdapter>::new(&query).load_page(conn)
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
