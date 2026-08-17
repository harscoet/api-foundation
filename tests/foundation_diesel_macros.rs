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
/// Generate the `apply_restriction` match body for a diesel `DieselField` impl.
///
/// Wraps the arms with `Ok(match (...) { ... })`, adds `query.filter(...)` around each
/// expression, and emits the fallback `_ => Err(QueryBuilderError)` automatically.
///
/// Note: a more compact `[num: gt, ge, ...]` DSL was attempted but Rust does not allow
/// macros to expand to match arms — `macros cannot expand to match arms` (stable limit).
/// The explicit arm form is the best achievable with `macro_rules!`.
#[allow(unused_macros)]
macro_rules! diesel_filter {
    (
        $query:expr, $field:expr, $comparator:expr, $value:expr,
        $(($f:pat, $c:pat, $v:pat) => $expr:expr),+ $(,)?
    ) => {
        Ok(match ($field, $comparator, $value) {
            $(
                ($f, $c, $v) => $query.filter($expr),
            )+
            _ => return Err(diesel::result::Error::QueryBuilderError(
                "unsupported filter combination".into(),
            )),
        })
    };
}

/// Generate the `field_cursor_value` match body for a `DieselList` impl.
///
/// Each `FieldVariant => [type] item.field` arm extracts the cursor value for a sort field.
/// Supported types: `str` (Option<String>, uses as_ref + clone), `f64` (Option<f64>, Copy).
///
/// Note: `@map` expands to an expression (the arm body), not a match arm — valid unlike `@arm`.
/// `item` is not a macro parameter; the accessor expression (`item.name`) closes over it directly.
#[allow(unused_macros)]
macro_rules! diesel_cursor_value {
    (
        $field:expr,
        $($variant:pat => [$ty:ident] $accessor:expr),+ $(,)?
    ) => {
        match $field {
            $(
                $variant => diesel_cursor_value!(@map $field, $accessor, $ty),
            )+
        }
    };
    (@map $field:expr, $accessor:expr, str) => {
        $accessor.as_ref().map(|v| api_foundation::pagination::CursorEntry {
            field_name: $field.to_string(),
            value: api_foundation::pagination::CursorValue::String(v.clone()),
        })
    };
    (@map $field:expr, $accessor:expr, f64) => {
        $accessor.map(|v| api_foundation::pagination::CursorEntry {
            field_name: $field.to_string(),
            value: api_foundation::pagination::CursorValue::Float64(v),
        })
    };
}

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

/// Generate the `load` body for a view-dispatched diesel SELECT.
///
/// Each `ViewVariant => Model => |row| mapping` arm expands to:
/// `q.select(Model::as_select()).limit(limit).load::<Model>(conn)?.into_iter().map(|row| mapping).collect()`
///
/// Note: `.select(Model::as_select())` is always emitted — for a full model this is
/// equivalent to no explicit select (all columns), so both views use the same pattern.
#[allow(unused_macros)]
macro_rules! diesel_load {
    (
        $q:expr, $view:expr, $limit:expr, $conn:expr,
        $($variant:pat => $model:ty => |$row:ident| $map:expr),+ $(,)?
    ) => {
        Ok(match $view {
            $(
                $variant => $q
                    .select(<$model>::as_select())
                    .limit($limit)
                    .load::<$model>($conn)?
                    .into_iter()
                    .map(|$row| $map)
                    .collect(),
            )+
        })
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
