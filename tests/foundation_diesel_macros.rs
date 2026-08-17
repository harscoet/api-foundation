/// Generate the `apply_field_ordering` match body for a diesel `BoxedQuery`.
///
/// Each `FieldVariant => column` pair expands to two match arms (Asc + Desc).
/// The tiebreaker column is appended to every ordering tuple.
///
/// # Example
/// ```ignore
/// fn apply_field_ordering<'a>(q: Self::Query<'a>, field: &ProductField, direction: &Direction) -> Self::Query<'a> {
///     diesel_order_by!(q, field, direction,
///         tiebreaker: products::id,
///         ProductField::Name  => products::name,
///         ProductField::Price => products::price,
///     )
/// }
/// ```
/// Generate the `apply_field_cursor` keyset WHERE clause for a diesel `BoxedQuery`.
///
/// Each `FieldVariant => column [type]` pair expands to a keyset filter for both
/// Asc and Desc directions. The tiebreaker column is appended to resolve ties.
/// Supported types: `f64` (uses `cursor_f64`), `str` (uses `cursor_string`).
///
/// # Example
/// ```ignore
/// fn apply_field_cursor<'a>(q: Self::Query<'a>, field: &ProductField, direction: &Direction, cursor: &[CursorEntry]) -> Self::Query<'a> {
///     diesel_cursor_filter!(q, field, direction, cursor,
///         tiebreaker: products::id,
///         ProductField::Price => products::price [f64],
///         ProductField::Name  => products::name  [str],
///     )
/// }
/// ```
#[allow(unused_macros)]
macro_rules! diesel_cursor_filter {
    (
        $q:expr, $field:expr, $direction:expr, $cursor:expr,
        tiebreaker: $tiebreaker:expr,
        $($variant:pat => $col:path [$ty:ident]),+ $(,)?
    ) => {{
        let id = foundation_diesel::cursor_i64($cursor, "id");
        match $field {
            $(
                $variant => {
                    let v = diesel_cursor_filter!(@extract $ty, $cursor, $field);
                    match $direction {
                        api_foundation::order_by::Direction::Asc  => $q.filter($col.gt(v.clone()).or($col.eq(v).and($tiebreaker.gt(id)))),
                        api_foundation::order_by::Direction::Desc => $q.filter($col.lt(v.clone()).or($col.eq(v).and($tiebreaker.gt(id)))),
                    }
                }
            )+
        }
    }};
    // Supported types: [f64] → cursor_f64, [str] → cursor_string.
    // To add a new type (e.g. [timestamp]), add a matching @extract arm here
    // and a corresponding cursor_* helper in foundation_diesel.
    (@extract f64, $cursor:expr, $field:expr) => {
        foundation_diesel::cursor_f64($cursor, Into::<&'static str>::into($field))
    };
    (@extract str, $cursor:expr, $field:expr) => {
        foundation_diesel::cursor_string($cursor, Into::<&'static str>::into($field))
    };
}

#[allow(unused_macros)]
macro_rules! diesel_order_by {
    (
        $q:expr, $field:expr, $direction:expr,
        tiebreaker: $tiebreaker:expr,
        $($variant:pat => $col:expr),+ $(,)?
    ) => {
        match ($field, $direction) {
            $(
                ($variant, api_foundation::order_by::Direction::Asc)  => $q.order(($col.asc(),  $tiebreaker.asc())),
                ($variant, api_foundation::order_by::Direction::Desc) => $q.order(($col.desc(), $tiebreaker.asc())),
            )+
        }
    };
}
