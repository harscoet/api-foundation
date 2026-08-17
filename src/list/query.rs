use crate::{
    error::Result,
    field::Field,
    filter::TypedFilter,
    order_by::OrderBy,
    pagination::{PageRequest, PageToken},
};

#[derive(Debug)]
pub struct ListQuery<F> {
    pub filter: Option<TypedFilter<F>>,
    pub order_by: Option<OrderBy<F>>,
    pub page_size: u32,
    pub cursor: Option<PageToken>,
    fingerprint: u64,
}

impl<F: Field> ListQuery<F> {
    pub fn build(
        filter: Option<&str>,
        order_by: Option<&str>,
        page_size: i32,
        page_token: Option<&str>,
    ) -> Result<Self> {
        let typed_filter = filter
            .filter(|s| !s.trim().is_empty())
            .map(TypedFilter::parse)
            .transpose()?;

        let typed_order_by = order_by
            .filter(|s| !s.trim().is_empty())
            .map(OrderBy::parse)
            .transpose()?;

        let page_request = PageRequest::new(page_size)?;

        let fingerprint = request_fingerprint(
            typed_filter.as_ref().map(|f| f.raw()),
            typed_order_by.as_ref().map(|o| o.raw()),
        );

        let cursor = page_token
            .filter(|s| !s.is_empty())
            .map(PageToken::decode)
            .transpose()?;

        if let Some(ref token) = cursor {
            token.verify_fingerprint(fingerprint)?;
        }

        Ok(Self {
            filter: typed_filter,
            order_by: typed_order_by,
            page_size: page_request.page_size,
            cursor,
            fingerprint,
        })
    }

    /// Fingerprint of this request's filter + order_by.
    /// Pass to [`PageToken::new`] when building the next page token.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// Deterministic FNV-1a hash of the canonical filter + order_by representation.
fn request_fingerprint(filter_raw: Option<&str>, order_by_raw: Option<&str>) -> u64 {
    let mut buf = String::new();
    buf.push_str(filter_raw.unwrap_or(""));
    buf.push('\0');
    buf.push_str(order_by_raw.unwrap_or(""));
    fnv1a_64(buf.as_bytes())
}

fn fnv1a_64(data: &[u8]) -> u64 {
    const OFFSET: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let mut h = OFFSET;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use aip_160::Comparator;

    use crate::{
        error::Error,
        field::Field as FieldTrait,
        pagination::{CursorEntry, CursorValue, PageToken},
    };

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Field {
        Name,
        Price,
    }

    impl std::str::FromStr for Field {
        type Err = ();
        fn from_str(s: &str) -> std::result::Result<Self, ()> {
            match s {
                "name" => Ok(Field::Name),
                "price" => Ok(Field::Price),
                _ => Err(()),
            }
        }
    }

    impl FieldTrait for Field {

        fn allowed_comparators(&self) -> &[Comparator] {
            match self {
                Field::Name => &[Comparator::Equal, Comparator::Has],
                Field::Price => &[
                    Comparator::Equal,
                    Comparator::LessThan,
                    Comparator::LessThanOrEqual,
                    Comparator::GreaterThan,
                    Comparator::GreaterThanOrEqual,
                ],
            }
        }

        fn is_orderable(&self) -> bool {
            true
        }
    }

    fn build(
        filter: Option<&str>,
        order_by: Option<&str>,
        page_size: i32,
        page_token: Option<&str>,
    ) -> Result<ListQuery<Field>> {
        ListQuery::build(filter, order_by, page_size, page_token)
    }

    #[test]
    fn minimal_build() {
        let q = build(None, None, 0, None).unwrap();
        assert!(q.filter.is_none());
        assert!(q.order_by.is_none());
        assert_eq!(q.page_size, 50);
        assert!(q.cursor.is_none());
    }

    #[test]
    fn with_filter_and_order_by() {
        let q = build(Some(r#"name = "foo""#), Some("price desc"), 10, None).unwrap();
        assert!(q.filter.is_some());
        assert!(q.order_by.is_some());
        assert_eq!(q.page_size, 10);
    }

    #[test]
    fn page_token_accepted_when_matching() {
        let q1 = build(Some(r#"name = "foo""#), Some("price desc"), 10, None).unwrap();
        let token = PageToken::new(
            vec![CursorEntry {
                field_name: "price".to_string(),
                value: CursorValue::Float64(9.99),
            }],
            q1.fingerprint(),
        );
        let encoded = token.encode();

        let q2 = build(Some(r#"name = "foo""#), Some("price desc"), 10, Some(&encoded)).unwrap();
        assert!(q2.cursor.is_some());
    }

    #[test]
    fn page_token_rejected_when_filter_changed() {
        let q1 = build(Some(r#"name = "foo""#), Some("price desc"), 10, None).unwrap();
        let token = PageToken::new(vec![], q1.fingerprint());
        let encoded = token.encode();

        // same order_by but different filter
        let err = build(Some(r#"name = "bar""#), Some("price desc"), 10, Some(&encoded))
            .unwrap_err();
        assert!(matches!(err, Error::PageTokenMismatch));
    }

    #[test]
    fn page_token_rejected_when_order_by_changed() {
        let q1 = build(None, Some("price desc"), 10, None).unwrap();
        let token = PageToken::new(vec![], q1.fingerprint());
        let encoded = token.encode();

        let err = build(None, Some("name asc"), 10, Some(&encoded)).unwrap_err();
        assert!(matches!(err, Error::PageTokenMismatch));
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let a = build(Some(r#"name = "foo""#), Some("price desc"), 10, None).unwrap();
        let b = build(Some(r#"name = "foo""#), Some("price desc"), 25, None).unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint()); // page_size not part of fingerprint
    }

    #[test]
    fn fingerprint_differs_on_different_inputs() {
        let a = build(Some(r#"name = "foo""#), None, 10, None).unwrap();
        let b = build(Some(r#"name = "bar""#), None, 10, None).unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn invalid_filter_propagates() {
        let err = build(Some("= = ="), None, 10, None).unwrap_err();
        assert!(matches!(err, Error::InvalidFilter(_)));
    }

    #[test]
    fn invalid_page_size_propagates() {
        let err = build(None, None, -5, None).unwrap_err();
        assert!(matches!(err, Error::InvalidPageSize(-5)));
    }

    #[test]
    fn corrupted_token_propagates() {
        let err = build(None, None, 10, Some("not_a_token!!!")).unwrap_err();
        assert!(matches!(err, Error::InvalidPageToken(_)));
    }

    #[test]
    fn empty_filter_string_treated_as_none() {
        let q = build(Some("   "), None, 10, None).unwrap();
        assert!(q.filter.is_none());
    }

    #[test]
    fn empty_order_by_string_treated_as_none() {
        let q = build(None, Some("   "), 10, None).unwrap();
        assert!(q.order_by.is_none());
    }
}
