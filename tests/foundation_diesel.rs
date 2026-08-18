//! Base of a future `api-foundation-diesel` crate.
//!
//! Provides the generic diesel integration layer for `api-foundation` types.
//! Entity-specific code (field→column mappings, ordering, cursors) lives in
//! the repository layer; only the AIP-160 filter AST traversal is generic here.

use api_foundation::{
    field::Field,
    filter::{Comparator, TypedExpression, TypedFilter, Value},
    list::ListQuery,
    order_by::{Direction, OrderBy},
    pagination::{CursorEntry, CursorValue, Page, PageToken},
};
use diesel::QueryResult;
use diesel::expression::BoxableExpression;
use diesel::pg::{Pg, PgConnection};
use diesel::query_dsl::methods::FilterDsl;
use diesel::sql_types::Bool;

/// Contract for a diesel-backed list repository.
///
/// The implementor provides entity-specific mappings (table, ordering, cursor,
/// view SELECT, and filter restrictions). [`diesel_list`] orchestrates the full
/// AIP-158 pagination loop generically — COUNT, cursor, page_size+1, token encoding.
///
/// ## Filter
///
/// `restriction_expr` converts a single AIP-160 restriction to a boxed diesel predicate.
/// [`build_predicate`] then recursively composes the full AST (AND / OR / NOT) before
/// applying everything with a single `.filter()` call.
///
/// This works because `dyn BoxableExpression<...> + 'static` is automatically `Send`
/// (diesel's blanket impl requires `where Self: Send`, so all implementors are Send and
/// the auto-trait propagates to the trait object). `And<Box<dyn ...>, Box<dyn ...>>`
/// is therefore itself boxable, enabling recursive composition.
///
/// ## Context
///
/// `type Context` carries server-side state (tenant ID, app config, …) that must be
/// applied to every query but never comes from the client request. Use `()` for entities
/// that need no extra context. [`diesel_list`] passes `ctx: L::Context` opaquely to
/// `base_query` and `count`; the impl decides what to do with it.
pub trait DieselList {
    type Field: Field;
    /// The entity's diesel table type — used to type-check boxed filter predicates.
    type Table: diesel::Table + 'static;
    /// The entity's `BoxedQuery` type. Set to `your_table::BoxedQuery<'a, Pg>`.
    type Query<'a>;
    type View;
    type Response;
    /// Server-side context passed to `base_query` and `count`. Use `()` when not needed.
    type Context;

    /// Build a base query with filter + context applied — no cursor, ordering, or limit.
    ///
    /// `ctx` carries server-side invariants (e.g. tenant filter, soft-delete exclusion)
    /// that must be enforced regardless of what the client sends.
    ///
    /// Typically: `foundation_diesel::base_query::<Self>(my_table::table.into_boxed(), filter)`
    fn base_query<'a>(
        filter: Option<&TypedFilter<Self::Field>>,
        ctx: &Self::Context,
    ) -> QueryResult<Self::Query<'a>>;

    /// Build a boxed diesel predicate for a single AIP-160 restriction.
    ///
    /// Values must be owned (clone strings, copy numbers) to satisfy `'static`.
    /// [`build_predicate`] composes the results generically for AND / OR / NOT.
    fn restriction_expr(
        field: &Self::Field,
        comparator: &Comparator,
        value: &Value,
    ) -> QueryResult<Box<dyn BoxableExpression<Self::Table, Pg, SqlType = Bool> + 'static>>;

    /// Total count with filter + context applied. Return `None` to skip the COUNT query.
    /// Default: `Ok(None)`. Override with `Ok(Some(Self::base_query(filter, ctx)?.count().get_result(conn)?))`.
    fn count(
        _filter: Option<&TypedFilter<Self::Field>>,
        _ctx: &Self::Context,
        _conn: &mut PgConnection,
    ) -> QueryResult<Option<i64>> {
        Ok(None)
    }

    /// Default ordering when no `order_by` is specified — typically the primary key.
    fn apply_tiebreaker_ordering<'a>(query: Self::Query<'a>) -> Self::Query<'a>;

    /// Apply ordering for a specific sort field + direction (tiebreaker included).
    fn apply_field_ordering<'a>(
        query: Self::Query<'a>,
        field: &Self::Field,
        direction: &Direction,
    ) -> Self::Query<'a>;

    /// Apply ORDER BY — dispatches to [`apply_tiebreaker_ordering`] or [`apply_field_ordering`].
    fn apply_ordering<'a>(
        query: Self::Query<'a>,
        order_by: Option<&OrderBy<Self::Field>>,
    ) -> Self::Query<'a> {
        match order_by.and_then(|o| o.clauses.first()) {
            None => Self::apply_tiebreaker_ordering(query),
            Some(clause) => Self::apply_field_ordering(query, &clause.field, &clause.direction),
        }
    }

    /// Keyset filter when there is no explicit ordering — typically `tiebreaker_col > cursor_id`.
    fn apply_tiebreaker_cursor<'a>(
        query: Self::Query<'a>,
        cursor: &[CursorEntry],
    ) -> Self::Query<'a>;

    /// Keyset filter for a specific sort field + direction + tiebreaker.
    fn apply_field_cursor<'a>(
        query: Self::Query<'a>,
        field: &Self::Field,
        direction: &Direction,
        cursor: &[CursorEntry],
    ) -> Self::Query<'a>;

    /// Apply the full keyset cursor — dispatches to [`apply_tiebreaker_cursor`] or
    /// [`apply_field_cursor`] based on whether ordering is active.
    fn apply_cursor<'a>(
        query: Self::Query<'a>,
        token: &PageToken,
        order_by: Option<&OrderBy<Self::Field>>,
    ) -> Self::Query<'a> {
        let cursor = token.cursor();
        match order_by.and_then(|ob| ob.clauses.first()) {
            None => Self::apply_tiebreaker_cursor(query, cursor),
            Some(clause) => {
                Self::apply_field_cursor(query, &clause.field, &clause.direction, cursor)
            }
        }
    }

    /// Execute the view-specific SELECT and map rows to `Response`.
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
    fn build_cursor(
        item: &Self::Response,
        order_by: Option<&OrderBy<Self::Field>>,
    ) -> Vec<CursorEntry> {
        let mut cursor: Vec<CursorEntry> = order_by
            .iter()
            .flat_map(|ob| &ob.clauses)
            .filter_map(|clause| Self::field_cursor_value(&clause.field, item))
            .collect();
        cursor.push(Self::tiebreaker(item));
        cursor
    }
}

/// Recursively build a boxed predicate from a typed filter AST.
///
/// AND / OR / NOT are composed generically using `.and()` / `.or()` / `dsl::not()`.
/// `+ 'static` (not `+ Send`) is the right bound — `dyn BoxableExpression + 'static`
/// is automatically `Send` because `BoxableExpression` requires `where Self: Send`,
/// so the auto-trait propagates to the trait object. Adding explicit `+ Send` would
/// create a distinct dyn type that diesel's blanket impls don't cover.
fn build_predicate<L: DieselList>(
    expr: &TypedExpression<L::Field>,
) -> QueryResult<Box<dyn BoxableExpression<L::Table, Pg, SqlType = Bool> + 'static>> {
    use diesel::BoolExpressionMethods;
    match expr {
        TypedExpression::And(l, r) => {
            let left = build_predicate::<L>(l)?;
            let right = build_predicate::<L>(r)?;
            Ok(Box::new(left.and(right)))
        }
        TypedExpression::Or(l, r) => {
            let left = build_predicate::<L>(l)?;
            let right = build_predicate::<L>(r)?;
            Ok(Box::new(left.or(right)))
        }
        TypedExpression::Not(e) => {
            let inner = build_predicate::<L>(e)?;
            Ok(Box::new(diesel::dsl::not(inner)))
        }
        TypedExpression::Restriction(r) => L::restriction_expr(&r.field, &r.comparator, &r.value),
    }
}

/// Apply a typed filter to a diesel query — builds the full predicate tree then calls `.filter()` once.
pub fn apply_filter<'a, L: DieselList>(
    query: L::Query<'a>,
    expr: &TypedExpression<L::Field>,
) -> QueryResult<L::Query<'a>>
where
    L::Query<'a>: FilterDsl<
            Box<dyn BoxableExpression<L::Table, Pg, SqlType = Bool> + 'static>,
            Output = L::Query<'a>,
        >,
{
    Ok(query.filter(build_predicate::<L>(expr)?))
}

/// Build a base query with only the filter applied — no cursor, ordering, or limit.
///
/// Pass the table's `into_boxed()` as `table_query`. The entire filter AST (AND/OR/NOT)
/// is compiled to a single boxed predicate, then applied in one `.filter()` call.
/// Reuse for both `COUNT(*)` and the paginated `SELECT` to avoid duplicating filter logic.
pub fn base_query<'a, L: DieselList>(
    table_query: L::Query<'a>,
    filter: Option<&TypedFilter<L::Field>>,
) -> QueryResult<L::Query<'a>>
where
    L::Query<'a>: FilterDsl<
            Box<dyn BoxableExpression<L::Table, Pg, SqlType = Bool> + 'static>,
            Output = L::Query<'a>,
        >,
{
    match filter {
        Some(f) => apply_filter::<L>(table_query, &f.expression),
        None => Ok(table_query),
    }
}

/// Extract an `i64` cursor value by field name. Returns `0` if absent.
pub fn cursor_i64(cursor: &[CursorEntry], field_name: &str) -> i64 {
    cursor
        .iter()
        .find(|e| e.field_name == field_name)
        .and_then(|e| match e.value {
            CursorValue::Int64(n) => Some(n),
            _ => None,
        })
        .unwrap_or(0)
}

/// Extract an `f64` cursor value by field name. Returns `f64::MIN` if absent.
pub fn cursor_f64(cursor: &[CursorEntry], field_name: &str) -> f64 {
    cursor
        .iter()
        .find(|e| e.field_name == field_name)
        .and_then(|e| match e.value {
            CursorValue::Float64(f) => Some(f),
            _ => None,
        })
        .unwrap_or(f64::MIN)
}

/// Extract a `String` cursor value by field name. Returns `""` if absent.
pub fn cursor_string(cursor: &[CursorEntry], field_name: &str) -> String {
    cursor
        .iter()
        .find(|e| e.field_name == field_name)
        .and_then(|e| match &e.value {
            CursorValue::String(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Generic AIP-158 pagination loop for any [`DieselList`] implementation.
pub fn diesel_list<L: DieselList>(
    conn: &mut PgConnection,
    query: ListQuery<L::Field>,
    view: L::View,
    ctx: L::Context,
) -> QueryResult<Page<L::Response>> {
    let total_size = L::count(query.filter.as_ref(), &ctx, conn)?;

    let mut q = L::base_query(query.filter.as_ref(), &ctx)?;
    if let Some(token) = &query.cursor {
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

    Ok(Page {
        items,
        next_page_token,
        total_size,
    })
}
