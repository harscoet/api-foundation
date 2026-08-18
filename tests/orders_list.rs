//! Integration test: api-foundation + diesel + PostgreSQL — Orders entity.
//!
//! Run with: cargo test --test orders_list
//! Requires: Docker daemon running, libpq installed.
//!
//! Demonstrates DieselList flexibility beyond the Products baseline:
//!   - Always-applied server-side filter: `deleted_at IS NULL` (soft delete)
//!   - JOIN in `load` — Full view joins customers, Minimal skips it
//!   - Manual `load` — the JOIN for Full view requires a custom select tuple

use diesel::pg::PgConnection;
use diesel::prelude::*;
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
use diesel::expression::BoxableExpression;
use diesel::pg::Pg;
use diesel::sql_types::Bool;
use foundation_diesel::DieselList;

// ── Diesel schema ─────────────────────────────────────────────────────────────

diesel::table! {
    customers (id) {
        id -> Int8,
        name -> Text,
    }
}

diesel::table! {
    orders (id) {
        id -> Int8,
        customer_id -> Int8,
        amount -> Double,
        status -> Text,
        created_at -> Int8,
        deleted_at -> Nullable<Int8>,
    }
}

diesel::joinable!(orders -> customers(customer_id));
diesel::allow_tables_to_appear_in_same_query!(customers, orders);

// ── OrderField ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, strum::Display, strum::EnumString, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
enum OrderField {
    Amount,
    Status,
    CreatedAt,
}

impl Field for OrderField {
    fn allowed_comparators(&self) -> &[Comparator] {
        match self {
            Self::Amount => &[
                Comparator::Equal,
                Comparator::LessThan,
                Comparator::LessThanOrEqual,
                Comparator::GreaterThan,
                Comparator::GreaterThanOrEqual,
            ],
            Self::Status => &[Comparator::Equal],
            Self::CreatedAt => &[
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

// ── OrderView (AIP-157) ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum OrderView {
    #[default]
    Full,    // joins customers → customer_name populated
    Minimal, // orders only — no JOIN, customer_name is None
}

// ── Response ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct OrderResponse {
    id: i64,
    customer_name: Option<String>,
    amount: f64,
    status: String,
    created_at: i64,
}

// ── Repository ────────────────────────────────────────────────────────────────

struct Orders;

impl DieselList for Orders {
    type Field = OrderField;
    type Table = orders::table;
    type View = OrderView;
    type Response = OrderResponse;
    type Query<'a> = orders::BoxedQuery<'a, diesel::pg::Pg>;

    // Always exclude soft-deleted rows before applying the caller's filter.
    fn base_query<'a>(filter: Option<&TypedFilter<Self::Field>>) -> QueryResult<Self::Query<'a>> {
        let q = orders::table.filter(orders::deleted_at.is_null()).into_boxed();
        foundation_diesel::base_query::<Self>(q, filter)
    }

    fn restriction_expr(
        field: &Self::Field,
        comparator: &Comparator,
        value: &Value,
    ) -> QueryResult<Box<dyn BoxableExpression<Self::Table, Pg, SqlType = Bool> + 'static>> {
        diesel_filter!(field, comparator, value,
            (OrderField::Amount,    Comparator::Equal,              Value::Number(n)) => orders::amount.eq(*n),
            (OrderField::Amount,    Comparator::GreaterThan,        Value::Number(n)) => orders::amount.gt(*n),
            (OrderField::Amount,    Comparator::GreaterThanOrEqual, Value::Number(n)) => orders::amount.ge(*n),
            (OrderField::Amount,    Comparator::LessThan,           Value::Number(n)) => orders::amount.lt(*n),
            (OrderField::Amount,    Comparator::LessThanOrEqual,    Value::Number(n)) => orders::amount.le(*n),
            (OrderField::Status,    Comparator::Equal,              Value::String(s)) => orders::status.eq(s.clone()),
            (OrderField::CreatedAt, Comparator::GreaterThan,        Value::Number(n)) => orders::created_at.gt(*n as i64),
            (OrderField::CreatedAt, Comparator::GreaterThanOrEqual, Value::Number(n)) => orders::created_at.ge(*n as i64),
            (OrderField::CreatedAt, Comparator::LessThan,           Value::Number(n)) => orders::created_at.lt(*n as i64),
            (OrderField::CreatedAt, Comparator::LessThanOrEqual,    Value::Number(n)) => orders::created_at.le(*n as i64),
        )
    }

    fn count(
        filter: Option<&TypedFilter<Self::Field>>,
        conn: &mut PgConnection,
    ) -> QueryResult<Option<i64>> {
        Ok(Some(Self::base_query(filter)?.count().get_result(conn)?))
    }

    fn apply_tiebreaker_ordering<'a>(q: Self::Query<'a>) -> Self::Query<'a> {
        q.order(orders::id.asc())
    }

    fn apply_field_ordering<'a>(
        q: Self::Query<'a>,
        field: &Self::Field,
        direction: &Direction,
    ) -> Self::Query<'a> {
        diesel_order_by!(q, field, direction,
            tiebreaker: orders::id,
            OrderField::Amount    => orders::amount,
            OrderField::Status    => orders::status,
            OrderField::CreatedAt => orders::created_at,
        )
    }

    fn apply_tiebreaker_cursor<'a>(q: Self::Query<'a>, cursor: &[CursorEntry]) -> Self::Query<'a> {
        q.filter(orders::id.gt(foundation_diesel::cursor_i64(cursor, "id")))
    }

    fn apply_field_cursor<'a>(
        q: Self::Query<'a>,
        field: &Self::Field,
        direction: &Direction,
        cursor: &[CursorEntry],
    ) -> Self::Query<'a> {
        diesel_cursor_filter!(q, field, direction, cursor,
            tiebreaker: orders::id,
            OrderField::Amount    => orders::amount     [f64],
            OrderField::Status    => orders::status     [str],
            OrderField::CreatedAt => orders::created_at [i64],
        )
    }

    // Written manually: `diesel_load!` assumes a single Selectable model; the join
    // for Full view requires a custom select tuple.
    fn load<'a>(
        q: Self::Query<'a>,
        view: &Self::View,
        limit: i64,
        conn: &mut PgConnection,
    ) -> QueryResult<Vec<Self::Response>> {
        match view {
            OrderView::Full => Ok(q
                .inner_join(customers::table)
                .select((
                    orders::id,
                    customers::name,
                    orders::amount,
                    orders::status,
                    orders::created_at,
                ))
                .limit(limit)
                .load::<(i64, String, f64, String, i64)>(conn)?
                .into_iter()
                .map(|(id, customer_name, amount, status, created_at)| OrderResponse {
                    id,
                    customer_name: Some(customer_name),
                    amount,
                    status,
                    created_at,
                })
                .collect()),
            OrderView::Minimal => Ok(q
                .select((orders::id, orders::amount, orders::status, orders::created_at))
                .limit(limit)
                .load::<(i64, f64, String, i64)>(conn)?
                .into_iter()
                .map(|(id, amount, status, created_at)| OrderResponse {
                    id,
                    customer_name: None,
                    amount,
                    status,
                    created_at,
                })
                .collect()),
        }
    }

    fn field_cursor_value(field: &Self::Field, item: &Self::Response) -> Option<CursorEntry> {
        diesel_cursor_value!(field,
            OrderField::Amount    => [f64] item.amount,
            OrderField::Status    => [str] item.status,
            OrderField::CreatedAt => [i64] item.created_at,
        )
    }

    fn tiebreaker(item: &Self::Response) -> CursorEntry {
        CursorEntry {
            field_name: "id".to_string(),
            value: CursorValue::Int64(item.id),
        }
    }
}

fn list_orders(
    conn: &mut PgConnection,
    query: ListQuery<OrderField>,
    view: OrderView,
) -> QueryResult<Page<OrderResponse>> {
    foundation_diesel::diesel_list::<Orders>(conn, query, view)
}

fn handle_list_orders(
    conn: &mut PgConnection,
    filter: &str,
    order_by: &str,
    page_size: i32,
    page_token: &str,
    view: OrderView,
) -> Result<Page<OrderResponse>, api_foundation::error::Error> {
    let query = ListQuery::<OrderField>::build(
        Some(filter),
        Some(order_by),
        page_size,
        Some(page_token),
    )?;
    list_orders(conn, query, view).map_err(|e| api_foundation::error::Error::InvalidFilter(e.to_string()))
}

// ── Test helpers ──────────────────────────────────────────────────────────────

fn setup(conn: &mut PgConnection) {
    diesel::sql_query(
        "CREATE TABLE IF NOT EXISTS customers (
            id   BIGSERIAL PRIMARY KEY,
            name TEXT NOT NULL
        )",
    )
    .execute(conn)
    .unwrap();

    diesel::sql_query(
        "CREATE TABLE IF NOT EXISTS orders (
            id          BIGSERIAL PRIMARY KEY,
            customer_id BIGINT NOT NULL REFERENCES customers(id),
            amount      DOUBLE PRECISION NOT NULL,
            status      TEXT NOT NULL,
            created_at  BIGINT NOT NULL,
            deleted_at  BIGINT
        )",
    )
    .execute(conn)
    .unwrap();

    diesel::sql_query("DELETE FROM orders").execute(conn).unwrap();
    diesel::sql_query("DELETE FROM customers").execute(conn).unwrap();

    diesel::sql_query(
        "INSERT INTO customers (name) VALUES ('Alice'), ('Bob'), ('Charlie')",
    )
    .execute(conn)
    .unwrap();

    // Live orders — 5 rows across 3 customers.
    diesel::sql_query(
        "INSERT INTO orders (customer_id, amount, status, created_at, deleted_at)
         SELECT c.id, vals.amount, vals.status, vals.created_at, NULL
         FROM (VALUES
             ('Alice',   100.0, 'pending',   1000),
             ('Bob',     200.0, 'shipped',   2000),
             ('Charlie', 300.0, 'delivered', 3000),
             ('Alice',   150.0, 'pending',   4000),
             ('Bob',     250.0, 'shipped',   5000)
         ) AS vals(name, amount, status, created_at)
         JOIN customers c ON c.name = vals.name",
    )
    .execute(conn)
    .unwrap();

    // Soft-deleted rows — must never appear in any result.
    diesel::sql_query(
        "INSERT INTO orders (customer_id, amount, status, created_at, deleted_at)
         SELECT c.id, 999.0, 'cancelled', 9000, 9999
         FROM customers c WHERE c.name = 'Charlie'",
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
async fn soft_delete_never_returned() {
    let (mut conn, _c) = pg_conn().await;
    setup(&mut conn);

    // total_size = 5 live orders; the deleted one (amount=999) is never counted or listed.
    let page = handle_list_orders(&mut conn, "", "", 50, "", OrderView::Full).unwrap();
    assert_eq!(page.total_size, Some(5));
    assert_eq!(page.items.len(), 5);
    assert!(page.items.iter().all(|o| o.amount < 500.0));
}

#[tokio::test]
async fn user_filter_combined_with_soft_delete() {
    let (mut conn, _c) = pg_conn().await;
    setup(&mut conn);

    // amount > 150 AND deleted_at IS NULL → 3 live orders (200, 300, 250).
    let page = handle_list_orders(&mut conn, "amount > 150", "", 50, "", OrderView::Full).unwrap();
    assert_eq!(page.total_size, Some(3));
    assert!(page.items.iter().all(|o| o.amount > 150.0));
}

#[tokio::test]
async fn full_view_populates_customer_name() {
    let (mut conn, _c) = pg_conn().await;
    setup(&mut conn);

    let page = handle_list_orders(&mut conn, "", "", 50, "", OrderView::Full).unwrap();
    assert!(page.items.iter().all(|o| o.customer_name.is_some()));
    let names: Vec<_> = page.items.iter().map(|o| o.customer_name.as_deref().unwrap()).collect();
    assert!(names.contains(&"Alice"));
    assert!(names.contains(&"Bob"));
    assert!(names.contains(&"Charlie"));
}

#[tokio::test]
async fn minimal_view_skips_join() {
    let (mut conn, _c) = pg_conn().await;
    setup(&mut conn);

    let page = handle_list_orders(&mut conn, "", "", 50, "", OrderView::Minimal).unwrap();
    assert_eq!(page.items.len(), 5);
    assert!(page.items.iter().all(|o| o.customer_name.is_none()));
}

#[tokio::test]
async fn pagination_excludes_deleted() {
    let (mut conn, _c) = pg_conn().await;
    setup(&mut conn);

    let p1 = handle_list_orders(&mut conn, "", "", 2, "", OrderView::Minimal).unwrap();
    assert_eq!(p1.items.len(), 2);
    assert_eq!(p1.total_size, Some(5));
    assert!(p1.next_page_token.is_some());

    let p2 = handle_list_orders(&mut conn, "", "", 2, p1.next_page_token.as_deref().unwrap(), OrderView::Minimal).unwrap();
    assert_eq!(p2.items.len(), 2);
    assert!(p2.next_page_token.is_some());

    let p3 = handle_list_orders(&mut conn, "", "", 2, p2.next_page_token.as_deref().unwrap(), OrderView::Minimal).unwrap();
    assert_eq!(p3.items.len(), 1);
    assert!(p3.next_page_token.is_none());

    let all_ids: Vec<i64> = [p1.items, p2.items, p3.items].concat().into_iter().map(|o| o.id).collect();
    assert_eq!(all_ids.len(), 5);
    let unique: std::collections::HashSet<_> = all_ids.iter().collect();
    assert_eq!(unique.len(), 5);
}

#[tokio::test]
async fn order_by_created_at_asc() {
    let (mut conn, _c) = pg_conn().await;
    setup(&mut conn);

    let p1 = handle_list_orders(&mut conn, "", "created_at asc", 2, "", OrderView::Minimal).unwrap();
    assert_eq!(p1.items[0].created_at, 1000);
    assert_eq!(p1.items[1].created_at, 2000);

    let p2 = handle_list_orders(&mut conn, "", "created_at asc", 2, p1.next_page_token.as_deref().unwrap(), OrderView::Minimal).unwrap();
    assert_eq!(p2.items[0].created_at, 3000);
    assert_eq!(p2.items[1].created_at, 4000);

    let p3 = handle_list_orders(&mut conn, "", "created_at asc", 2, p2.next_page_token.as_deref().unwrap(), OrderView::Minimal).unwrap();
    assert_eq!(p3.items[0].created_at, 5000);
    assert!(p3.next_page_token.is_none());
}

#[tokio::test]
async fn token_mismatch_rejected() {
    let (mut conn, _c) = pg_conn().await;
    setup(&mut conn);

    let p1 = handle_list_orders(&mut conn, "amount > 100", "amount asc", 2, "", OrderView::Full).unwrap();
    let token = p1.next_page_token.as_deref().unwrap();

    // Different filter → rejected.
    let err = handle_list_orders(&mut conn, "amount > 200", "amount asc", 2, token, OrderView::Full).unwrap_err();
    assert!(matches!(err, api_foundation::error::Error::PageTokenMismatch));

    // Different order_by → rejected.
    let err = handle_list_orders(&mut conn, "amount > 100", "amount desc", 2, token, OrderView::Full).unwrap_err();
    assert!(matches!(err, api_foundation::error::Error::PageTokenMismatch));

    // Same parameters → accepted.
    assert!(handle_list_orders(&mut conn, "amount > 100", "amount asc", 2, token, OrderView::Full).is_ok());
}
