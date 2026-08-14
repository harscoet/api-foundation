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

    #[test]
    fn field_unknown_name_returns_none() {
        assert!(F::from_field_name("unknown").is_none());
    }

    #[test]
    fn field_known_name_returns_variant() {
        assert_eq!(F::from_field_name("name"), Some(F::Name));
    }
}
