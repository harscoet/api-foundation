use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid filter: {0}")]
    InvalidFilter(String),

    #[error("unknown field: {field:?}")]
    UnknownField { field: String },

    #[error("comparator '{comparator}' is not allowed for field '{field}'")]
    DisallowedComparator { field: String, comparator: String },

    #[error("invalid order_by: {0}")]
    InvalidOrderBy(String),

    #[error("page_size must be non-negative, got {0}")]
    InvalidPageSize(i32),

    #[error("invalid page_token: {0}")]
    InvalidPageToken(String),

    #[error("page_token does not match request parameters")]
    PageTokenMismatch,
}

pub type Result<T> = std::result::Result<T, Error>;
