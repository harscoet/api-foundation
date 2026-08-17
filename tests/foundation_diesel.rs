//! Base of a future `api-foundation-diesel` crate.
//!
//! Provides the generic diesel integration layer for `api-foundation` types.
//! Entity-specific code (field→column mappings, ordering, cursors) lives in
//! the repository layer; only the AIP-160 filter recursion is generic here.

use api_foundation::{
    field::Field,
    filter::{Comparator, TypedExpression, TypedFilter, Value},
};
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
