use aip_160::ast::{Expression, Filter};

use crate::{
    error::{Error, Result},
    field::Field,
};

use super::typed::{TypedExpression, TypedFilter, TypedRestriction};

pub fn from_ast<F: Field>(filter: Filter, raw: String) -> Result<TypedFilter<F>> {
    let expression = expression(filter.expression)?;
    Ok(TypedFilter { expression, raw })
}

fn expression<F: Field>(expr: Expression) -> Result<TypedExpression<F>> {
    match expr {
        Expression::And(l, r) => Ok(TypedExpression::And(
            Box::new(expression(*l)?),
            Box::new(expression(*r)?),
        )),
        Expression::Or(l, r) => Ok(TypedExpression::Or(
            Box::new(expression(*l)?),
            Box::new(expression(*r)?),
        )),
        Expression::Not(e) => Ok(TypedExpression::Not(Box::new(expression(*e)?))),
        Expression::Restriction(r) => {
            let field = r
                .field
                .parse::<F>()
                .ok()
                .ok_or_else(|| Error::UnknownField {
                    field: r.field.clone(),
                })?;
            if !field.allowed_comparators().contains(&r.comparator) {
                return Err(Error::DisallowedComparator {
                    field: r.field,
                    comparator: r.comparator.to_string(),
                });
            }
            Ok(TypedExpression::Restriction(TypedRestriction {
                field,
                comparator: r.comparator,
                value: r.value,
            }))
        }
        Expression::Sequence(s) => Err(Error::InvalidFilter(format!(
            "bare field path '{}' without operator is not supported",
            s.parts.join(".")
        ))),
    }
}
