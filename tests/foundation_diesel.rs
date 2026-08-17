//! Base of a future `api-foundation-diesel` crate.
//!
//! Provides the generic diesel integration layer for `api-foundation` types.
//! Entity-specific code (field→column mappings, ordering, cursors) lives in
//! the repository layer; only the AIP-160 filter recursion is generic here.

use api_foundation::{
    field::Field,
    filter::{Comparator, TypedExpression, TypedFilter, Value},
    list::ListQuery,
    order_by::{Direction, OrderBy},
    pagination::{CursorEntry, CursorValue, Page, PageToken},
};
use diesel::pg::PgConnection;
use diesel::QueryResult;

/// Extension of [`Field`] for diesel-backed repositories.
///
/// The implementor provides only `apply_restriction` — the specific
/// (field, comparator, value) tuples valid for their entity. The generic
/// `And` recursion is handled by [`apply_filter`].
pub trait DieselField: Field {
    type Query<'a>;

    fn apply_restriction<'a>(
        query: Self::Query<'a>,
        field: &Self,
        comparator: &Comparator,
        value: &'a Value,
    ) -> QueryResult<Self::Query<'a>>;
}

/// Apply a typed filter expression to a diesel query.
///
/// Handles `And` recursion generically. `Or`/`Not` return an error — they
/// require `BoxableExpression` and will be supported in a future version.
pub fn apply_filter<'a, F: DieselField>(
    query: F::Query<'a>,
    expr: &'a TypedExpression<F>,
) -> QueryResult<F::Query<'a>> {
    match expr {
        TypedExpression::And(l, r) => apply_filter(apply_filter(query, l)?, r),
        TypedExpression::Restriction(r) => {
            F::apply_restriction(query, &r.field, &r.comparator, &r.value)
        }
        _ => Err(diesel::result::Error::QueryBuilderError(
            "OR/NOT not supported — requires BoxableExpression (future api-foundation-diesel)".into(),
        )),
    }
}

/// Extract an `i64` cursor value by field name. Returns `0` if absent (defensive fallback).
pub fn cursor_i64(cursor: &[CursorEntry], field_name: &str) -> i64 {
    cursor
        .iter()
        .find(|e| e.field_name == field_name)
        .and_then(|e| if let CursorValue::Int64(n) = e.value { Some(n) } else { None })
        .unwrap_or(0)
}

/// Extract an `f64` cursor value by field name. Returns `f64::MIN` if absent.
pub fn cursor_f64(cursor: &[CursorEntry], field_name: &str) -> f64 {
    cursor
        .iter()
        .find(|e| e.field_name == field_name)
        .and_then(|e| if let CursorValue::Float64(f) = e.value { Some(f) } else { None })
        .unwrap_or(f64::MIN)
}

/// Extract a `String` cursor value by field name. Returns `""` if absent.
pub fn cursor_string(cursor: &[CursorEntry], field_name: &str) -> String {
    cursor
        .iter()
        .find(|e| e.field_name == field_name)
        .and_then(|e| if let CursorValue::String(s) = &e.value { Some(s.clone()) } else { None })
        .unwrap_or_default()
}

/// Build a base query with only the filter applied — no cursor, ordering, or limit.
///
/// Pass the table's `into_boxed()` as `table_query`; the filter is applied on top.
/// Reuse for both `COUNT(*)` and the paginated `SELECT` to avoid duplicating filter logic.
pub fn base_query<'a, F: DieselField>(
    table_query: F::Query<'a>,
    filter: Option<&'a TypedFilter<F>>,
) -> QueryResult<F::Query<'a>> {
    match filter {
        Some(f) => apply_filter(table_query, &f.expression),
        None => Ok(table_query),
    }
}

/// Contract for a diesel-backed list repository.
///
/// The implementor provides entity-specific mappings (table, ordering, cursor,
/// view SELECT). [`diesel_list`] orchestrates the full AIP-158 pagination loop
/// generically — COUNT, cursor, page_size+1, token encoding.
pub trait DieselList {
    type Field: DieselField;
    type View;
    type Response;
    /// The entity's `BoxedQuery` type. Set to `your_table::BoxedQuery<'a, Pg>`.
    type Query<'a>;

    /// Table initialization + filter application.
    /// Typically: `foundation_diesel::base_query(my_table::table.into_boxed(), filter)`
    fn base_query<'a>(
        filter: Option<&'a TypedFilter<Self::Field>>,
    ) -> QueryResult<Self::Query<'a>>;

    /// Total count with filter only. Return `None` to skip the COUNT query entirely.
    /// Default: `Ok(None)`. Override with `Ok(Some(Self::base_query(filter)?.count().get_result(conn)?))`.
    fn count(
        _filter: Option<&TypedFilter<Self::Field>>,
        _conn: &mut PgConnection,
    ) -> QueryResult<Option<i64>> {
        Ok(None)
    }

    /// Default ordering when no `order_by` is specified — typically the primary key.
    fn apply_tiebreaker_ordering<'a>(query: Self::Query<'a>) -> Self::Query<'a>
    where
        Self: 'a;

    /// Apply ordering for a specific sort field + direction (tiebreaker included).
    fn apply_field_ordering<'a>(
        query: Self::Query<'a>,
        field: &Self::Field,
        direction: &Direction,
    ) -> Self::Query<'a>
    where
        Self: 'a;

    /// Apply ORDER BY — dispatches to [`apply_tiebreaker_ordering`] or [`apply_field_ordering`].
    fn apply_ordering<'a, 'b>(
        query: Self::Query<'a>,
        order_by: Option<&'b OrderBy<Self::Field>>,
    ) -> Self::Query<'a>
    where
        Self: 'a,
    {
        match order_by.and_then(|o| o.clauses.first()) {
            None => Self::apply_tiebreaker_ordering(query),
            Some(clause) => Self::apply_field_ordering(query, &clause.field, &clause.direction),
        }
    }

    /// Keyset filter when there is no explicit ordering — typically `tiebreaker_col > cursor_id`.
    fn apply_tiebreaker_cursor<'a>(query: Self::Query<'a>, cursor: &[CursorEntry]) -> Self::Query<'a>
    where
        Self: 'a;

    /// Keyset filter for a specific sort field + direction + tiebreaker.
    fn apply_field_cursor<'a>(
        query: Self::Query<'a>,
        field: &Self::Field,
        direction: &Direction,
        cursor: &[CursorEntry],
    ) -> Self::Query<'a>
    where
        Self: 'a;

    /// Apply the full keyset cursor — dispatches to [`apply_tiebreaker_cursor`] or
    /// [`apply_field_cursor`] based on whether ordering is active.
    fn apply_cursor<'a, 'b>(
        query: Self::Query<'a>,
        token: &'b PageToken,
        order_by: Option<&'b OrderBy<Self::Field>>,
    ) -> Self::Query<'a>
    where
        Self: 'a,
    {
        let cursor = token.cursor();
        match order_by.and_then(|ob| ob.clauses.first()) {
            None => Self::apply_tiebreaker_cursor(query, cursor),
            Some(clause) => Self::apply_field_cursor(query, &clause.field, &clause.direction, cursor),
        }
    }

    /// Execute the view-specific SELECT and map rows to `Response`.
    /// This is the only method that varies meaningfully between views.
    fn load<'a>(
        query: Self::Query<'a>,
        view: &Self::View,
        limit: i64,
        conn: &mut PgConnection,
    ) -> QueryResult<Vec<Self::Response>>;

    /// Extract the cursor entry for a given sort field from a response item.
    /// Return `None` if the field value is absent (e.g. not loaded by this view).
    fn field_cursor_value(field: &Self::Field, item: &Self::Response) -> Option<CursorEntry>;

    /// Stable tiebreaker appended after all sort fields — typically the primary key.
    fn tiebreaker(item: &Self::Response) -> CursorEntry;

    /// Build a keyset cursor from the last item in a page.
    fn build_cursor(item: &Self::Response, order_by: Option<&OrderBy<Self::Field>>) -> Vec<CursorEntry> {
        let mut cursor: Vec<CursorEntry> = order_by
            .iter()
            .flat_map(|ob| &ob.clauses)
            .filter_map(|clause| Self::field_cursor_value(&clause.field, item))
            .collect();
        cursor.push(Self::tiebreaker(item));
        cursor
    }
}

/// Generic AIP-158 pagination loop for any [`DieselList`] implementation.
///
/// Handles COUNT, cursor application, page_size+1 trick, has_next detection,
/// and next-page token encoding. The entity only implements [`DieselList`].
pub fn diesel_list<L: DieselList>(
    conn: &mut PgConnection,
    query: ListQuery<L::Field>,
    view: L::View,
) -> QueryResult<Page<L::Response>> {
    let total_size = L::count(query.filter.as_ref(), conn)?;

    let mut q = L::base_query(query.filter.as_ref())?;
    if let Some(ref token) = query.cursor {
        q = L::apply_cursor(q, token, query.order_by.as_ref());
    }
    q = L::apply_ordering(q, query.order_by.as_ref());

    let limit = query.page_size as i64 + 1;
    let mut items = L::load(q, &view, limit, conn)?;

    let has_next = items.len() > query.page_size as usize;
    if has_next {
        items.pop();
    }

    let next_page_token = has_next.then(|| {
        PageToken::new(
            L::build_cursor(items.last().unwrap(), query.order_by.as_ref()),
            query.fingerprint(),
        )
        .encode()
    });

    Ok(Page { items, next_page_token, total_size })
}
