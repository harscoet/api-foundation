use aip_160::{Comparator, Value};

use crate::error::{Error, Result};

use super::{validate, FilterableField};

#[derive(Debug)]
pub struct TypedFilter<F> {
    pub expression: TypedExpression<F>,
    pub(super) raw: String,
}

#[derive(Debug)]
pub enum TypedExpression<F> {
    And(Box<TypedExpression<F>>, Box<TypedExpression<F>>),
    Or(Box<TypedExpression<F>>, Box<TypedExpression<F>>),
    Not(Box<TypedExpression<F>>),
    Restriction(TypedRestriction<F>),
}

#[derive(Debug)]
pub struct TypedRestriction<F> {
    pub field: F,
    pub comparator: Comparator,
    pub value: Value,
}

impl<F: FilterableField> TypedFilter<F> {
    pub fn parse(input: &str) -> Result<Self> {
        let raw = input.to_string();
        let ast = aip_160::parse_filter(input)
            .map_err(|e| Error::InvalidFilter(e.to_string()))?;
        validate::from_ast(ast, raw)
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }
}
