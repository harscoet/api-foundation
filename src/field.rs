use aip_160::Comparator;
use std::str::FromStr;

/// Implemented once per resource's field enum.
///
/// Capabilities default to the safe minimum: not filterable, not orderable.
/// Override only what the field actually supports.
///
/// The `FromStr` bound maps the protobuf field string to the typed enum variant.
/// Return `Err` for unknown names — callers surface this as `Error::UnknownField`.
pub trait Field: Sized + FromStr {
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

    impl std::str::FromStr for F {
        type Err = ();
        fn from_str(s: &str) -> Result<Self, ()> {
            match s {
                "name" => Ok(F::Name),
                "price" => Ok(F::Price),
                _ => Err(()),
            }
        }
    }

    impl Field for F {
        fn allowed_comparators(&self) -> &[Comparator] {
            &[]
        }
    }

    #[test]
    fn field_unknown_name_returns_none() {
        assert!("unknown".parse::<F>().is_err());
    }

    #[test]
    fn field_known_name_returns_variant() {
        assert_eq!("name".parse::<F>().ok(), Some(F::Name));
    }
}
