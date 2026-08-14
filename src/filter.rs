mod validate;
pub mod typed;

pub use aip_160::{Comparator, Value};
pub use typed::{TypedExpression, TypedFilter, TypedRestriction};

#[cfg(test)]
mod tests {
    use aip_160::Comparator;

    use crate::field::Field;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum F {
        Name,
        Price,
        Active,
    }

    impl Field for F {
        fn from_field_name(name: &str) -> Option<Self> {
            match name {
                "name" => Some(F::Name),
                "price" => Some(F::Price),
                "active" => Some(F::Active),
                _ => None,
            }
        }

        fn allowed_comparators(&self) -> &[Comparator] {
            match self {
                F::Name => &[Comparator::Equal, Comparator::NotEqual, Comparator::Has],
                F::Price => &[
                    Comparator::Equal,
                    Comparator::NotEqual,
                    Comparator::LessThan,
                    Comparator::LessThanOrEqual,
                    Comparator::GreaterThan,
                    Comparator::GreaterThanOrEqual,
                ],
                F::Active => &[Comparator::Equal, Comparator::NotEqual],
            }
        }
    }

    #[test]
    fn simple_equality() {
        let f = TypedFilter::<F>::parse(r#"name = "foo""#).unwrap();
        let TypedExpression::Restriction(r) = f.expression else {
            panic!("expected restriction");
        };
        assert_eq!(r.field, F::Name);
        assert_eq!(r.comparator, Comparator::Equal);
    }

    #[test]
    fn and_expression() {
        let f = TypedFilter::<F>::parse(r#"name = "foo" AND active = true"#).unwrap();
        assert!(matches!(f.expression, TypedExpression::And(_, _)));
    }

    #[test]
    fn or_expression() {
        let f = TypedFilter::<F>::parse(r#"name = "a" OR name = "b""#).unwrap();
        assert!(matches!(f.expression, TypedExpression::Or(_, _)));
    }

    #[test]
    fn not_expression() {
        let f = TypedFilter::<F>::parse("NOT active = true").unwrap();
        assert!(matches!(f.expression, TypedExpression::Not(_)));
    }

    #[test]
    fn nested_and_or() {
        TypedFilter::<F>::parse(r#"name = "a" AND (active = true OR price > 10)"#).unwrap();
    }

    #[test]
    fn unknown_field_is_error() {
        use crate::error::Error;
        let err = TypedFilter::<F>::parse(r#"unknown = "x""#).unwrap_err();
        assert!(matches!(err, Error::UnknownField { field } if field == "unknown"));
    }

    #[test]
    fn disallowed_comparator_is_error() {
        use crate::error::Error;
        let err = TypedFilter::<F>::parse("name > 0").unwrap_err();
        assert!(matches!(err, Error::DisallowedComparator { field, .. } if field == "name"));
    }

    #[test]
    fn parse_error_on_invalid_syntax() {
        use crate::error::Error;
        let err = TypedFilter::<F>::parse("= = =").unwrap_err();
        assert!(matches!(err, Error::InvalidFilter(_)));
    }

    #[test]
    fn raw_is_preserved() {
        let input = r#"name = "foo""#;
        let f = TypedFilter::<F>::parse(input).unwrap();
        assert_eq!(f.raw(), input);
    }
}
