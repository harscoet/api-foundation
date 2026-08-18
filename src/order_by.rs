use crate::{
    error::{Error, Result},
    field::Field,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direction {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub struct OrderClause<F> {
    pub field: F,
    pub direction: Direction,
}

#[derive(Debug, Clone)]
pub struct OrderBy<F> {
    pub clauses: Vec<OrderClause<F>>,
    raw: String,
}

impl<F: Field> OrderBy<F> {
    pub fn parse(input: &str) -> Result<Self> {
        if input.trim().is_empty() {
            return Err(Error::InvalidOrderBy("empty order_by string".to_string()));
        }

        let mut clauses = Vec::new();

        for part in input.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err(Error::InvalidOrderBy(format!("empty clause in: {input:?}")));
            }

            let (field_name, direction) = if let Some(pos) = part.rfind(' ') {
                let last = &part[pos + 1..];
                let before = part[..pos].trim_end();
                match last {
                    "asc" => (before, Direction::Asc),
                    "desc" => (before, Direction::Desc),
                    _ => (part, Direction::Asc),
                }
            } else {
                (part, Direction::Asc)
            };

            let field = field_name
                .parse::<F>()
                .ok()
                .ok_or_else(|| Error::UnknownField {
                    field: field_name.to_string(),
                })?;

            if !field.is_orderable() {
                return Err(Error::InvalidOrderBy(format!(
                    "field '{field_name}' is not orderable"
                )));
            }

            clauses.push(OrderClause { field, direction });
        }

        Ok(Self {
            clauses,
            raw: input.to_string(),
        })
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn is_empty(&self) -> bool {
        self.clauses.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use aip_160::Comparator;

    use crate::field::Field;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum F {
        Name,
        Price,
        CreatedAt,
        Description, // filterable but not orderable — to test the new guard
    }

    impl std::str::FromStr for F {
        type Err = ();
        fn from_str(s: &str) -> std::result::Result<Self, ()> {
            match s {
                "name" => Ok(F::Name),
                "price" => Ok(F::Price),
                "created_at" => Ok(F::CreatedAt),
                "description" => Ok(F::Description),
                _ => Err(()),
            }
        }
    }

    impl Field for F {
        fn allowed_comparators(&self) -> &[Comparator] {
            match self {
                F::Description => &[Comparator::Has],
                _ => &[],
            }
        }

        fn is_orderable(&self) -> bool {
            matches!(self, F::Name | F::Price | F::CreatedAt)
        }
    }

    #[test]
    fn single_field_default_asc() {
        let ob = OrderBy::<F>::parse("name").unwrap();
        assert_eq!(ob.clauses.len(), 1);
        assert_eq!(ob.clauses[0].field, F::Name);
        assert_eq!(ob.clauses[0].direction, Direction::Asc);
    }

    #[test]
    fn single_field_explicit_asc() {
        let ob = OrderBy::<F>::parse("name asc").unwrap();
        assert_eq!(ob.clauses[0].direction, Direction::Asc);
    }

    #[test]
    fn single_field_desc() {
        let ob = OrderBy::<F>::parse("price desc").unwrap();
        assert_eq!(ob.clauses[0].field, F::Price);
        assert_eq!(ob.clauses[0].direction, Direction::Desc);
    }

    #[test]
    fn multiple_fields() {
        let ob = OrderBy::<F>::parse("name asc, price desc").unwrap();
        assert_eq!(ob.clauses.len(), 2);
        assert_eq!(ob.clauses[0].field, F::Name);
        assert_eq!(ob.clauses[0].direction, Direction::Asc);
        assert_eq!(ob.clauses[1].field, F::Price);
        assert_eq!(ob.clauses[1].direction, Direction::Desc);
    }

    #[test]
    fn multiple_fields_no_direction() {
        let ob = OrderBy::<F>::parse("name, price, created_at").unwrap();
        assert_eq!(ob.clauses.len(), 3);
        assert!(ob.clauses.iter().all(|c| c.direction == Direction::Asc));
    }

    #[test]
    fn whitespace_tolerance() {
        let ob = OrderBy::<F>::parse("  name  desc  ,  price  asc  ").unwrap();
        assert_eq!(ob.clauses[0].field, F::Name);
        assert_eq!(ob.clauses[0].direction, Direction::Desc);
        assert_eq!(ob.clauses[1].field, F::Price);
        assert_eq!(ob.clauses[1].direction, Direction::Asc);
    }

    #[test]
    fn unknown_field_is_error() {
        let err = OrderBy::<F>::parse("unknown desc").unwrap_err();
        assert!(matches!(err, Error::UnknownField { field } if field == "unknown"));
    }

    #[test]
    fn non_orderable_field_is_error() {
        let err = OrderBy::<F>::parse("description asc").unwrap_err();
        assert!(matches!(err, Error::InvalidOrderBy(msg) if msg.contains("not orderable")));
    }

    #[test]
    fn empty_string_is_error() {
        assert!(matches!(
            OrderBy::<F>::parse("").unwrap_err(),
            Error::InvalidOrderBy(_)
        ));
    }

    #[test]
    fn empty_clause_is_error() {
        assert!(matches!(
            OrderBy::<F>::parse("name,,price").unwrap_err(),
            Error::InvalidOrderBy(_)
        ));
    }

    #[test]
    fn raw_is_preserved() {
        let input = "name asc, price desc";
        let ob = OrderBy::<F>::parse(input).unwrap();
        assert_eq!(ob.raw(), input);
    }
}
