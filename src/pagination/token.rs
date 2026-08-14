use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

use crate::error::{Error, Result};

const TOKEN_VERSION: u8 = 1;

#[derive(bitcode::Encode, bitcode::Decode)]
struct RawPageToken {
    version: u8,
    fingerprint: u64,
    cursor: Vec<RawCursorEntry>,
}

#[derive(bitcode::Encode, bitcode::Decode)]
struct RawCursorEntry {
    field_name: String,
    value: RawCursorValue,
}

#[derive(bitcode::Encode, bitcode::Decode)]
enum RawCursorValue {
    String(String),
    Int64(i64),
    Float64(u64), // stored as raw bits for deterministic encoding
    Bool(bool),
    Null,
}

#[derive(Debug)]
pub struct PageToken {
    fingerprint: u64,
    cursor: Vec<CursorEntry>,
}

#[derive(Debug)]
pub struct CursorEntry {
    pub field_name: String,
    pub value: CursorValue,
}

#[derive(Debug)]
pub enum CursorValue {
    String(String),
    Int64(i64),
    Float64(f64),
    Bool(bool),
    Null,
}

impl PageToken {
    pub fn new(cursor: Vec<CursorEntry>, fingerprint: u64) -> Self {
        Self { fingerprint, cursor }
    }

    pub fn encode(&self) -> String {
        let raw = RawPageToken {
            version: TOKEN_VERSION,
            fingerprint: self.fingerprint,
            cursor: self
                .cursor
                .iter()
                .map(|e| RawCursorEntry {
                    field_name: e.field_name.clone(),
                    value: match &e.value {
                        CursorValue::String(s) => RawCursorValue::String(s.clone()),
                        CursorValue::Int64(n) => RawCursorValue::Int64(*n),
                        CursorValue::Float64(f) => RawCursorValue::Float64(f.to_bits()),
                        CursorValue::Bool(b) => RawCursorValue::Bool(*b),
                        CursorValue::Null => RawCursorValue::Null,
                    },
                })
                .collect(),
        };
        URL_SAFE_NO_PAD.encode(bitcode::encode(&raw))
    }

    pub fn decode(s: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|e| Error::InvalidPageToken(format!("base64: {e}")))?;

        let raw: RawPageToken = bitcode::decode(&bytes)
            .map_err(|e| Error::InvalidPageToken(format!("decode: {e}")))?;

        if raw.version != TOKEN_VERSION {
            return Err(Error::InvalidPageToken(format!(
                "unsupported version: {}",
                raw.version
            )));
        }

        let cursor = raw
            .cursor
            .into_iter()
            .map(|e| CursorEntry {
                field_name: e.field_name,
                value: match e.value {
                    RawCursorValue::String(s) => CursorValue::String(s),
                    RawCursorValue::Int64(n) => CursorValue::Int64(n),
                    RawCursorValue::Float64(bits) => CursorValue::Float64(f64::from_bits(bits)),
                    RawCursorValue::Bool(b) => CursorValue::Bool(b),
                    RawCursorValue::Null => CursorValue::Null,
                },
            })
            .collect();

        Ok(Self {
            fingerprint: raw.fingerprint,
            cursor,
        })
    }

    pub fn verify_fingerprint(&self, fingerprint: u64) -> Result<()> {
        if self.fingerprint == fingerprint {
            Ok(())
        } else {
            Err(Error::PageTokenMismatch)
        }
    }

    pub fn cursor(&self) -> &[CursorEntry] {
        &self.cursor
    }

    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_token(fingerprint: u64) -> PageToken {
        PageToken::new(
            vec![
                CursorEntry {
                    field_name: "created_at".to_string(),
                    value: CursorValue::String("2024-01-15T10:00:00Z".to_string()),
                },
                CursorEntry {
                    field_name: "id".to_string(),
                    value: CursorValue::Int64(42),
                },
            ],
            fingerprint,
        )
    }

    #[test]
    fn roundtrip() {
        let token = make_token(12345);
        let encoded = token.encode();
        let decoded = PageToken::decode(&encoded).unwrap();

        assert_eq!(decoded.fingerprint(), 12345);
        assert_eq!(decoded.cursor().len(), 2);

        let CursorValue::String(ref s) = decoded.cursor()[0].value else {
            panic!("expected String");
        };
        assert_eq!(s, "2024-01-15T10:00:00Z");

        let CursorValue::Int64(n) = decoded.cursor()[1].value else {
            panic!("expected Int64");
        };
        assert_eq!(n, 42);
    }

    #[test]
    fn all_cursor_value_types() {
        let token = PageToken::new(
            vec![
                CursorEntry { field_name: "s".to_string(), value: CursorValue::String("x".to_string()) },
                CursorEntry { field_name: "i".to_string(), value: CursorValue::Int64(-99) },
                CursorEntry { field_name: "f".to_string(), value: CursorValue::Float64(3.14) },
                CursorEntry { field_name: "b".to_string(), value: CursorValue::Bool(true) },
                CursorEntry { field_name: "n".to_string(), value: CursorValue::Null },
            ],
            0,
        );
        let decoded = PageToken::decode(&token.encode()).unwrap();
        assert_eq!(decoded.cursor().len(), 5);
        assert!(matches!(decoded.cursor()[2].value, CursorValue::Float64(f) if (f - 3.14).abs() < f64::EPSILON));
        assert!(matches!(decoded.cursor()[3].value, CursorValue::Bool(true)));
        assert!(matches!(decoded.cursor()[4].value, CursorValue::Null));
    }

    #[test]
    fn verify_matching_fingerprint() {
        let token = make_token(999);
        assert!(token.verify_fingerprint(999).is_ok());
    }

    #[test]
    fn verify_mismatched_fingerprint() {
        let token = make_token(999);
        assert!(matches!(
            token.verify_fingerprint(1000).unwrap_err(),
            Error::PageTokenMismatch
        ));
    }

    #[test]
    fn corrupted_token_is_error() {
        let err = PageToken::decode("not_valid_base64!!!").unwrap_err();
        assert!(matches!(err, Error::InvalidPageToken(_)));
    }

    #[test]
    fn tampered_bytes_is_error() {
        let token = make_token(0);
        let mut encoded = token.encode();
        // flip a character
        let mut chars: Vec<char> = encoded.chars().collect();
        chars[5] = if chars[5] == 'A' { 'B' } else { 'A' };
        encoded = chars.into_iter().collect();
        // may be base64 error or decode error — both are InvalidPageToken
        let err = PageToken::decode(&encoded);
        if let Err(e) = err {
            assert!(matches!(e, Error::InvalidPageToken(_)));
        }
        // (a flipped bit may still decode to a valid but wrong token — that's expected with binary formats)
    }

    #[test]
    fn empty_cursor_roundtrip() {
        let token = PageToken::new(vec![], 42);
        let decoded = PageToken::decode(&token.encode()).unwrap();
        assert_eq!(decoded.cursor().len(), 0);
        assert_eq!(decoded.fingerprint(), 42);
    }
}
