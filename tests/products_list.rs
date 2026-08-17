//! Integration test: api-foundation + diesel + PostgreSQL (testcontainers).
//!
//! Run with: cargo test --test products_list
//! Requires: Docker daemon running, libpq installed.
//!
//! Shows how little code a developer writes for a complete AIP-compliant List
//! method: only `ProductField` + `ProductView` + the diesel translation.
//! Everything else (parsing, validation, token encoding, consistency checks)
//! is api-foundation.

use diesel::prelude::*;
use diesel::pg::PgConnection;
use testcontainers_modules::{postgres::Postgres, testcontainers::runners::AsyncRunner};

mod foundation_diesel;
#[macro_use]
mod foundation_diesel_macros;

use api_foundation::{
    field::Field,
    filter::{Comparator, TypedFilter, Value},
    list::ListQuery,
    order_by::Direction,
    pagination::{CursorEntry, CursorValue, Page},
};
use foundation_diesel::{DieselField, DieselList};

// ── Diesel schema ─────────────────────────────────────────────────────────────

diesel::table! {
    products (id) {
        id -> Int8,
        name -> Text,
        price -> Double,
    }
}

// ── DB models ─────────────────────────────────────────────────────────────────

// Full row — loaded when view = Full.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = products)]
struct Product {
    id: i64,
    name: String,
    price: f64,
}

// Partial row — loaded when view = Basic. Only id and name are fetched from DB.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = products)]
struct ProductBasic {
    id: i64,
    name: String,
}

// ── Response type ─────────────────────────────────────────────────────────────
//
// In a real gRPC handler this would be the proto message. Option<T> reflects
// view semantics: a field absent from the view is returned at its zero value
// (empty string / 0.0 in proto). The view determines what the DB loads.

#[derive(Debug, Clone)]
struct ProductResponse {
    id: i64,           // always returned — resource identifier
    name: Option<String>,
    price: Option<f64>,
}

// ── ProductField ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, strum::Display, strum::EnumString, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
enum ProductField {
    Name,
    Price,
}

impl Field for ProductField {
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

    fn is_orderable(&self) -> bool {
        true
    }
}

impl DieselField for ProductField {
    type Query<'a> = products::BoxedQuery<'a, diesel::pg::Pg>;

    fn apply_restriction<'a>(
        query: Self::Query<'a>,
        field: &Self,
        comparator: &Comparator,
        value: &'a Value,
    ) -> diesel::QueryResult<Self::Query<'a>> {
        Ok(match (field, comparator, value) {
            (Self::Name, Comparator::Equal, Value::String(s)) => {
                query.filter(products::name.eq(s.clone()))
            }
            (Self::Price, Comparator::GreaterThan, Value::Number(n)) => {
                query.filter(products::price.gt(n))
            }
            (Self::Price, Comparator::GreaterThanOrEqual, Value::Number(n)) => {
                query.filter(products::price.ge(n))
            }
            (Self::Price, Comparator::LessThan, Value::Number(n)) => {
                query.filter(products::price.lt(n))
            }
            (Self::Price, Comparator::LessThanOrEqual, Value::Number(n)) => {
                query.filter(products::price.le(n))
            }
            _ => return Err(diesel::result::Error::QueryBuilderError(
                "unsupported filter combination".into(),
            )),
        })
    }
}

// ── ProductView (AIP-157) ─────────────────────────────────────────────────────
//
// The client selects a view; the server uses it to determine which columns to
// fetch. Full (default) loads everything. Basic omits price — useful when the
// caller only needs names and doesn't want to pay for the extra column.
//
// Note: ordering by a field requires that field to be present in the view.
// E.g., `view=Basic, order_by="price"` produces an incomplete cursor.
// In production, validate this before executing the query.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum ProductView {
    #[default]
    Full,   // proto value 0 — unspecified, returns all fields
    Basic,  // proto value 1 — id + name only (no price fetched from DB)
}

impl ProductView {
    fn from_proto(value: i32) -> Self {
        match value {
            1 => Self::Basic,
            _ => Self::Full,
        }
    }
}

// ── Repository ────────────────────────────────────────────────────────────────
//
// `Products` implements `DieselList` — the entity-specific mappings (table,
// ordering, cursor, view SELECT). `foundation_diesel::diesel_list` orchestrates
// the full AIP-158 pagination loop generically.

struct Products;

impl DieselList for Products {
    type Field = ProductField;
    type View = ProductView;
    type Response = ProductResponse;
    type Query<'a> = products::BoxedQuery<'a, diesel::pg::Pg>;

    fn base_query<'a>(filter: Option<&'a TypedFilter<ProductField>>) -> QueryResult<Self::Query<'a>> {
        foundation_diesel::base_query(products::table.into_boxed(), filter)
    }

    fn count(filter: Option<&TypedFilter<ProductField>>, conn: &mut PgConnection) -> QueryResult<Option<i64>> {
        Ok(Some(Self::base_query(filter)?.count().get_result(conn)?))
    }

    fn apply_tiebreaker_ordering<'a>(q: Self::Query<'a>) -> Self::Query<'a> {
        q.order(products::id.asc())
    }

    fn apply_field_ordering<'a>(q: Self::Query<'a>, field: &ProductField, direction: &Direction) -> Self::Query<'a> {
        diesel_order_by!(q, field, direction,
            tiebreaker: products::id,
            ProductField::Name  => products::name,
            ProductField::Price => products::price,
        )
    }

    fn apply_tiebreaker_cursor<'a>(q: Self::Query<'a>, cursor: &[CursorEntry]) -> Self::Query<'a> {
        q.filter(products::id.gt(foundation_diesel::cursor_i64(cursor, "id")))
    }

    fn apply_field_cursor<'a>(q: Self::Query<'a>, field: &ProductField, direction: &Direction, cursor: &[CursorEntry]) -> Self::Query<'a>
    {
        let id = foundation_diesel::cursor_i64(cursor, "id");
        let name: &'static str = field.into();
        match field {
            ProductField::Price => {
                let v = foundation_diesel::cursor_f64(cursor, name);
                match direction {
                    Direction::Asc  => q.filter(products::price.gt(v).or(products::price.eq(v).and(products::id.gt(id)))),
                    Direction::Desc => q.filter(products::price.lt(v).or(products::price.eq(v).and(products::id.gt(id)))),
                }
            }
            ProductField::Name => {
                let v = foundation_diesel::cursor_string(cursor, name);
                match direction {
                    Direction::Asc  => q.filter(products::name.gt(v.clone()).or(products::name.eq(v).and(products::id.gt(id)))),
                    Direction::Desc => q.filter(products::name.lt(v.clone()).or(products::name.eq(v).and(products::id.gt(id)))),
                }
            }
        }
    }

    fn load<'a>(q: Self::Query<'a>, view: &ProductView, limit: i64, conn: &mut PgConnection) -> QueryResult<Vec<ProductResponse>> {
        Ok(match view {
            ProductView::Basic => q
                .select(ProductBasic::as_select())
                .limit(limit)
                .load::<ProductBasic>(conn)?
                .into_iter()
                .map(|p| ProductResponse { id: p.id, name: Some(p.name), price: None })
                .collect(),
            ProductView::Full => q
                .limit(limit)
                .load::<Product>(conn)?
                .into_iter()
                .map(|p| ProductResponse { id: p.id, name: Some(p.name), price: Some(p.price) })
                .collect(),
        })
    }

    fn field_cursor_value(field: &ProductField, item: &ProductResponse) -> Option<CursorEntry> {
        match field {
            ProductField::Name => item.name.as_ref().map(|s| CursorEntry {
                field_name: field.to_string(),
                value: CursorValue::String(s.clone()),
            }),
            ProductField::Price => item.price.map(|p| CursorEntry {
                field_name: field.to_string(),
                value: CursorValue::Float64(p),
            }),
        }
    }

    fn tiebreaker(item: &ProductResponse) -> CursorEntry {
        CursorEntry { field_name: "id".to_string(), value: CursorValue::Int64(item.id) }
    }
}

fn list_products(
    conn: &mut PgConnection,
    query: ListQuery<ProductField>,
    view: ProductView,
) -> QueryResult<Page<ProductResponse>> {
    foundation_diesel::diesel_list::<Products>(conn, query, view)
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
//           // view parsed from the proto enum integer
//           let view = ProductView::from_proto(req.view);
//           let query = ListQuery::<ProductField>::build(
//               Some(&req.filter), Some(&req.order_by),
//               req.page_size, Some(&req.page_token),
//           )?;
//           // View is applied at the SQL level — only the selected columns are loaded.
//           let page = self.repo.list(query, view).await?;
//           let items = page.items.into_iter().map(|p| ProductProto {
//               id:    p.id,
//               name:  p.name.unwrap_or_default(),
//               price: p.price.unwrap_or_default(),
//               ..Default::default()
//           }).collect();
//           Ok(Response::new(ListProductsResponse { items, ... }))
//       }
//   }

fn handle_list_products(
    conn: &mut PgConnection,
    filter: &str,
    order_by: &str,
    page_size: i32,
    page_token: &str,
    view: i32,
) -> Result<Page<ProductResponse>, api_foundation::error::Error> {
    let query = ListQuery::<ProductField>::build(
        Some(filter),
        Some(order_by),
        page_size,
        Some(page_token),
    )?;
    let view = ProductView::from_proto(view);
    list_products(conn, query, view)
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

    let p1 = handle_list_products(&mut conn, "", "", 2, "", 0).unwrap();
    assert_eq!(p1.items.len(), 2);
    assert!(p1.next_page_token.is_some());

    let p2 = handle_list_products(&mut conn, "", "", 2, p1.next_page_token.as_deref().unwrap(), 0).unwrap();
    assert_eq!(p2.items.len(), 2);
    assert!(p2.next_page_token.is_some());

    let p3 = handle_list_products(&mut conn, "", "", 2, p2.next_page_token.as_deref().unwrap(), 0).unwrap();
    assert_eq!(p3.items.len(), 1);
    assert!(p3.next_page_token.is_none());

    assert_eq!(p1.total_size, Some(5));
    assert_eq!(p2.total_size, Some(5));
    assert_eq!(p3.total_size, Some(5));

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

    let page = handle_list_products(&mut conn, "price > 25", "", 10, "", 0).unwrap();
    assert_eq!(page.items.len(), 3); // 30, 40, 50
    assert!(page.items.iter().all(|p| p.price.unwrap() > 25.0));
    assert!(page.next_page_token.is_none());
    assert_eq!(page.total_size, Some(3));
}

#[tokio::test]
async fn filter_combined_with_pagination() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    let p1 = handle_list_products(&mut conn, "price > 10", "", 2, "", 0).unwrap();
    assert_eq!(p1.items.len(), 2);
    assert_eq!(p1.total_size, Some(4));
    assert!(p1.next_page_token.is_some());

    let p2 = handle_list_products(&mut conn, "price > 10", "", 2, p1.next_page_token.as_deref().unwrap(), 0).unwrap();
    assert_eq!(p2.items.len(), 2);
    assert_eq!(p2.total_size, Some(4));
    assert!(p2.next_page_token.is_none());
}

#[tokio::test]
async fn pagination_with_ordering_desc() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    let p1 = handle_list_products(&mut conn, "", "price desc", 2, "", 0).unwrap();
    assert_eq!(p1.items[0].price, Some(50.0));
    assert_eq!(p1.items[1].price, Some(40.0));

    let p2 = handle_list_products(&mut conn, "", "price desc", 2, p1.next_page_token.as_deref().unwrap(), 0).unwrap();
    assert_eq!(p2.items[0].price, Some(30.0));
    assert_eq!(p2.items[1].price, Some(20.0));

    let p3 = handle_list_products(&mut conn, "", "price desc", 2, p2.next_page_token.as_deref().unwrap(), 0).unwrap();
    assert_eq!(p3.items[0].price, Some(10.0));
    assert!(p3.next_page_token.is_none());
}

#[tokio::test]
async fn token_mismatch_is_rejected() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    let p1 = handle_list_products(&mut conn, "price > 10", "price asc", 2, "", 0).unwrap();
    let token = p1.next_page_token.as_deref().unwrap();

    let err = handle_list_products(&mut conn, "price > 20", "price asc", 2, token, 0).unwrap_err();
    assert!(matches!(err, api_foundation::error::Error::PageTokenMismatch));

    let err = handle_list_products(&mut conn, "price > 10", "price desc", 2, token, 0).unwrap_err();
    assert!(matches!(err, api_foundation::error::Error::PageTokenMismatch));

    // Same parameters — accepted
    assert!(handle_list_products(&mut conn, "price > 10", "price asc", 2, token, 0).is_ok());

    // Different page_size — accepted (AIP-158: page_size not part of fingerprint)
    assert!(handle_list_products(&mut conn, "price > 10", "price asc", 5, token, 0).is_ok());
}

#[tokio::test]
async fn unknown_field_rejected_before_db() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    let err = handle_list_products(&mut conn, r#"secret = "x""#, "", 10, "", 0).unwrap_err();
    assert!(matches!(
        err,
        api_foundation::error::Error::UnknownField { field } if field == "secret"
    ));
}

#[tokio::test]
async fn disallowed_comparator_rejected_before_db() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    let err = handle_list_products(&mut conn, "name > 0", "", 10, "", 0).unwrap_err();
    assert!(matches!(
        err,
        api_foundation::error::Error::DisallowedComparator { field, .. } if field == "name"
    ));
}

#[tokio::test]
async fn full_view_loads_all_fields() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    // view=0 (unspecified) → Full — both name and price are loaded from DB
    let page = handle_list_products(&mut conn, "", "", 10, "", 0).unwrap();
    assert!(page.items.iter().all(|p| p.name.is_some() && p.price.is_some()));
}

#[tokio::test]
async fn basic_view_loads_only_id_and_name() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    // view=1 → Basic — price column is not fetched from the DB
    let page = handle_list_products(&mut conn, "", "", 10, "", 1).unwrap();
    assert_eq!(page.items.len(), 5);
    assert!(page.items.iter().all(|p| p.name.is_some()));
    assert!(page.items.iter().all(|p| p.price.is_none()));
    assert!(page.items.iter().all(|p| p.id > 0));
}

#[tokio::test]
async fn basic_view_with_filter_and_pagination() {
    let (mut conn, _container) = pg_conn().await;
    setup(&mut conn);

    // Filter still applies even in Basic view — but only id and name are returned
    let p1 = handle_list_products(&mut conn, "price > 10", "", 2, "", 1).unwrap();
    assert_eq!(p1.items.len(), 2);
    assert_eq!(p1.total_size, Some(4));
    assert!(p1.items.iter().all(|p| p.price.is_none()));
    assert!(p1.next_page_token.is_some());

    // Pagination token works: same view, same filter
    let p2 = handle_list_products(&mut conn, "price > 10", "", 2, p1.next_page_token.as_deref().unwrap(), 1).unwrap();
    assert_eq!(p2.items.len(), 2);
    assert!(p2.next_page_token.is_none());

    // All 4 matching products returned across both pages, each exactly once
    let all_names: Vec<String> = [p1.items, p2.items]
        .concat()
        .into_iter()
        .map(|p| p.name.unwrap())
        .collect();
    assert_eq!(all_names.len(), 4);
    let unique: std::collections::HashSet<_> = all_names.iter().collect();
    assert_eq!(unique.len(), 4);
}
