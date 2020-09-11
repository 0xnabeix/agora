use crate::expressions::*;
use crate::matching::{match_directives, match_selections};
use graphql_parser::query::{Directive, FragmentDefinition, Query, Selection, SelectionSet};
use num_bigint::BigInt;
use std::any::Any;
use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub struct Document<'a> {
    pub statements: Vec<Statement<'a>>,
}

#[derive(Debug, PartialEq)]
pub struct Statement<'a> {
    pub predicate: Predicate<'a>,
    pub cost_expr: LinearExpression,
}

impl<'s> Statement<'s> {
    pub fn try_cost<'a, 'a2: 'a>(
        &self,
        query: &'a TopLevelQueryItem<'a2>,
        fragments: &'a [FragmentDefinition<'a2, &'a2 str>],
        captures: &mut Captures,
    ) -> Result<Option<BigInt>, ()> {
        if self.predicate.match_with_vars(query, fragments, captures)? {
            Ok(Some(self.cost_expr.eval(captures)?))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Predicate<'a> {
    pub graphql: TopLevelQueryItem<'a>,
    pub when_clause: Option<WhenClause>,
}

#[derive(Debug, PartialEq)]
pub enum TopLevelQueryItem<'a> {
    Directive(Directive<'a, &'a str>),
    Selection(Selection<'a, &'a str>),
}

impl<'a> TopLevelQueryItem<'a> {
    fn match_with_vars<'o, 'o2: 'o, 'f, 'f2: 'f>(
        &self,
        other: &'o TopLevelQueryItem<'o2>,
        fragments: &'f [FragmentDefinition<'f2, &'f2 str>],
        capture: &mut Captures,
    ) -> Result<bool, ()> {
        match (self, other) {
            (Self::Directive(s), TopLevelQueryItem::Directive(o)) => {
                match_directives(s, o, fragments, capture)
            }
            (Self::Selection(s), TopLevelQueryItem::Selection(o)) => {
                match_selections(s, o, fragments, capture)
            }
            _ => Ok(false),
        }
    }

    pub fn from_query(query: Query<'a, &'a str>) -> Vec<Self> {
        let Query {
            directives,
            selection_set,
            ..
        } = query;
        let mut result = Vec::new();
        for directive in directives.into_iter() {
            result.push(TopLevelQueryItem::Directive(directive));
        }
        for selection in selection_set.items.into_iter() {
            result.push(TopLevelQueryItem::Selection(selection));
        }
        result
    }

    pub fn from_selection_set(selection_set: SelectionSet<'a, &'a str>) -> Vec<Self> {
        let mut result = Vec::new();
        for selection in selection_set.items.into_iter() {
            result.push(TopLevelQueryItem::Selection(selection));
        }
        result
    }
}

impl Predicate<'_> {
    pub fn match_with_vars<'a, 'a2: 'a>(
        &self,
        item: &'a TopLevelQueryItem<'a2>,
        fragments: &'a [FragmentDefinition<'a2, &'a2 str>],
        captures: &mut Captures,
    ) -> Result<bool, ()> {
        captures.clear();

        if !(self.graphql.match_with_vars(item, fragments, captures)?) {
            return Ok(false);
        }

        if let Some(when_clause) = &self.when_clause {
            if !(when_clause.condition.eval(captures)?) {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

#[derive(Debug, PartialEq)]
pub struct WhenClause {
    pub condition: Condition,
}

// TODO: (Performance) It would be simple to fold consts
// by just evaluating each side without captures and seeing if it comes up with a value.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum LinearExpression {
    Const(Const<BigInt>),
    Variable(Variable<BigInt>),
    BinaryExpression(Box<BinaryExpression<AnyLinearOperator, LinearExpression>>),
}

impl Expression for LinearExpression {
    type Type = BigInt;
    fn eval(&self, captures: &Captures) -> Result<Self::Type, ()> {
        match self {
            Self::Const(inner) => inner.eval(captures),
            Self::Variable(inner) => inner.eval(captures),
            Self::BinaryExpression(inner) => inner.eval(captures),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Condition {
    Comparison(BinaryExpression<AnyComparison, LinearExpression>),
    Boolean(Box<BinaryExpression<AnyBooleanOp, Condition>>),
    Variable(Variable<bool>),
    Const(Const<bool>),
}

impl Expression for Condition {
    type Type = bool;
    fn eval(&self, captures: &Captures) -> Result<Self::Type, ()> {
        match self {
            Self::Comparison(inner) => inner.eval(captures),
            Self::Boolean(inner) => inner.eval(captures),
            Self::Variable(inner) => inner.eval(captures),
            Self::Const(inner) => inner.eval(captures),
        }
    }
}

#[derive(Default, Debug)]
pub struct Captures {
    values: HashMap<String, Box<dyn Any>>,
}

impl Captures {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn insert<T: 'static>(&mut self, name: impl Into<String>, value: T) {
        self.values.insert(name.into(), Box::new(value));
    }

    pub fn get<T: 'static>(&self, name: &str) -> Option<Result<&T, ()>> {
        // TODO: This resolves a mismatch in whether or not the $ is kept in the variable name,
        // but we want to fix that at the parser level instead.
        let name = name.trim_start_matches('$');

        match self.values.get(name) {
            Some(v) => match v.downcast_ref() {
                Some(v) => Some(Ok(v)),
                None => Some(Err(())),
            },
            None => None,
        }
    }

    pub fn clear(&mut self) {
        self.values.clear()
    }
}

#[cfg(test)]
mod test_helpers {
    use super::*;
    impl From<()> for Captures {
        fn from(_: ()) -> Captures {
            Captures::new()
        }
    }

    impl<T0: 'static> From<(&'_ str, T0)> for Captures {
        fn from(value: (&'_ str, T0)) -> Captures {
            let mut v = Captures::new();
            v.insert(value.0, value.1);
            v
        }
    }

    impl<T0: 'static, T1: 'static> From<((&'_ str, T0), (&'_ str, T1))> for Captures {
        fn from(value: ((&'_ str, T0), (&'_ str, T1))) -> Captures {
            let mut v = Captures::new();
            v.insert((value.0).0, (value.0).1);
            v.insert((value.1).0, (value.1).1);
            v
        }
    }
}
