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
