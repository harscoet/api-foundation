use aip_160::Comparator;

/// Implemented once per resource's field enum.
///
/// Capabilities default to the safe minimum: not filterable, not orderable.
/// Override only what the field actually supports.
pub trait Field: Sized {
    /// Map a protobuf field string to the typed enum variant.
    /// Return `None` for unknown names — callers surface this as an error.
    fn from_field_name(name: &str) -> Option<Self>;

    /// Comparators allowed when this field appears in a filter expression.
    /// Default `&[]` makes the field non-filterable: any comparator is rejected.
    fn allowed_comparators(&self) -> &[Comparator] {
        &[]
    }

    /// Whether this field may appear in an `order_by` clause.
    fn is_orderable(&self) -> bool {
        false
    }
}

/// Named predefined projection for SQL-level field selection (AIP-157 views).
///
/// Views are coarse-grained subsets defined by the API. The client selects a
/// view; the server determines which columns to fetch from the data store.
///
/// The default view (proto value `0` / unspecified) must return all fields.
pub trait View: Sized + Default {
    /// Parse from the protobuf enum integer value.
    /// `0` (unspecified) must map to `Self::default()`.
    fn from_proto(value: i32) -> Self;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum F {
        Name,
        Price,
    }

    impl Field for F {
        fn from_field_name(name: &str) -> Option<Self> {
            match name {
                "name" => Some(F::Name),
                "price" => Some(F::Price),
                _ => None,
            }
        }

        fn allowed_comparators(&self) -> &[Comparator] {
            &[]
        }
    }

    #[derive(Debug, Default, PartialEq, Eq)]
    enum V {
        #[default]
        Full,
        Basic,
    }

    impl View for V {
        fn from_proto(value: i32) -> Self {
            match value {
                1 => V::Basic,
                _ => V::Full,
            }
        }
    }

    #[test]
    fn view_unspecified_maps_to_default() {
        assert_eq!(V::from_proto(0), V::Full);
    }

    #[test]
    fn view_known_value_maps_correctly() {
        assert_eq!(V::from_proto(1), V::Basic);
    }

    #[test]
    fn view_unknown_value_falls_back_to_default() {
        assert_eq!(V::from_proto(99), V::Full);
    }

    #[test]
    fn field_unknown_name_returns_none() {
        assert!(F::from_field_name("unknown").is_none());
    }

    #[test]
    fn field_known_name_returns_variant() {
        assert_eq!(F::from_field_name("name"), Some(F::Name));
    }
}
