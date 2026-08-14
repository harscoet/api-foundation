use aip_160::Comparator;

use crate::error::{Error, Result};

/// Implemented once per resource's field enum.
///
/// Capabilities default to the safe minimum: not filterable, not orderable,
/// maskable. Override only what the field actually supports.
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

    /// Whether this field may appear in a `read_mask` (AIP-157).
    /// Defaults to `true`; set to `false` for internal-only fields.
    fn is_maskable(&self) -> bool {
        true
    }
}

/// Parsed and validated AIP-157 field mask.
///
/// An empty mask means "all fields" — consistent with AIP-157 which states
/// that an absent `read_mask` returns the full resource.
///
/// ```text
/// // Client sends: read_mask = "name,price"
/// let mask = FieldMask::<ProductField>::parse("name,price")?;
/// mask.includes(&ProductField::Price); // true
/// mask.includes(&ProductField::CreatedAt); // false
/// ```
#[derive(Debug, Clone)]
pub struct FieldMask<F> {
    // Empty == all fields. Non-empty == explicit allowlist.
    fields: Vec<F>,
}

impl<F: Field> FieldMask<F> {
    /// Parse a comma-separated mask string.
    ///
    /// Empty or whitespace-only input is valid and means "all fields".
    /// Returns an error for unknown field names or non-maskable fields.
    pub fn parse(input: &str) -> Result<Self> {
        if input.trim().is_empty() {
            return Ok(Self { fields: Vec::new() });
        }

        let fields = input
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|name| {
                let f = F::from_field_name(name).ok_or_else(|| {
                    Error::InvalidFieldMask(format!("unknown field: {name}"))
                })?;
                if !f.is_maskable() {
                    return Err(Error::InvalidFieldMask(format!(
                        "field '{name}' is not maskable"
                    )));
                }
                Ok(f)
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self { fields })
    }

    /// Returns `true` when the mask is empty (all fields requested).
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The explicitly requested fields. Empty slice means all fields.
    pub fn fields(&self) -> &[F] {
        &self.fields
    }
}

impl<F: Field + PartialEq> FieldMask<F> {
    /// Whether `field` should be included in the response.
    ///
    /// Always `true` when the mask is empty (all fields requested).
    pub fn includes(&self, field: &F) -> bool {
        self.fields.is_empty() || self.fields.contains(field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum F {
        Name,
        Price,
        Internal,
    }

    impl Field for F {
        fn from_field_name(name: &str) -> Option<Self> {
            match name {
                "name" => Some(F::Name),
                "price" => Some(F::Price),
                "internal" => Some(F::Internal),
                _ => None,
            }
        }

        fn allowed_comparators(&self) -> &[Comparator] {
            &[]
        }

        fn is_maskable(&self) -> bool {
            !matches!(self, F::Internal)
        }
    }

    #[test]
    fn empty_mask_means_all_fields() {
        let mask = FieldMask::<F>::parse("").unwrap();
        assert!(mask.is_empty());
        assert!(mask.includes(&F::Name));
        assert!(mask.includes(&F::Price));
    }

    #[test]
    fn whitespace_mask_means_all_fields() {
        let mask = FieldMask::<F>::parse("   ").unwrap();
        assert!(mask.is_empty());
    }

    #[test]
    fn valid_fields_are_parsed() {
        let mask = FieldMask::<F>::parse("name,price").unwrap();
        assert!(!mask.is_empty());
        assert_eq!(mask.fields(), &[F::Name, F::Price]);
    }

    #[test]
    fn includes_respects_explicit_mask() {
        let mask = FieldMask::<F>::parse("name").unwrap();
        assert!(mask.includes(&F::Name));
        assert!(!mask.includes(&F::Price));
    }

    #[test]
    fn whitespace_around_field_names_is_trimmed() {
        let mask = FieldMask::<F>::parse(" name , price ").unwrap();
        assert_eq!(mask.fields(), &[F::Name, F::Price]);
    }

    #[test]
    fn unknown_field_is_error() {
        let err = FieldMask::<F>::parse("name,unknown").unwrap_err();
        assert!(matches!(err, Error::InvalidFieldMask(msg) if msg.contains("unknown")));
    }

    #[test]
    fn non_maskable_field_is_error() {
        let err = FieldMask::<F>::parse("internal").unwrap_err();
        assert!(matches!(err, Error::InvalidFieldMask(msg) if msg.contains("internal")));
    }
}
