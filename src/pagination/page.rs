use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct PageRequest {
    pub page_size: u32,
}

impl PageRequest {
    pub const DEFAULT_PAGE_SIZE: u32 = 50;
    pub const MAX_PAGE_SIZE: u32 = 1000;

    pub fn new(page_size: i32) -> Result<Self> {
        if page_size < 0 {
            return Err(Error::InvalidPageSize(page_size));
        }
        let size = if page_size == 0 {
            Self::DEFAULT_PAGE_SIZE
        } else {
            (page_size as u32).min(Self::MAX_PAGE_SIZE)
        };
        Ok(Self { page_size: size })
    }
}

pub struct Page<T> {
    pub items: Vec<T>,
    pub next_page_token: Option<String>,
    pub total_size: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn zero_uses_default() {
        let r = PageRequest::new(0).unwrap();
        assert_eq!(r.page_size, PageRequest::DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn explicit_size() {
        let r = PageRequest::new(25).unwrap();
        assert_eq!(r.page_size, 25);
    }

    #[test]
    fn over_max_is_coerced() {
        let r = PageRequest::new(9999).unwrap();
        assert_eq!(r.page_size, PageRequest::MAX_PAGE_SIZE);
    }

    #[test]
    fn negative_is_error() {
        assert!(matches!(
            PageRequest::new(-1).unwrap_err(),
            Error::InvalidPageSize(-1)
        ));
    }
}
