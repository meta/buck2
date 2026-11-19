/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//!
//! A math computation built on dice.
//! The math computation will calculate a series of addition equations resolving variables.
//! e.g. `x = 1; y = 2, a = x + y; b = a + a; eval(b)`;

use std::convert::Infallible;
use std::str::FromStr;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::DiceComputations;
use dice::DiceKeyDyn;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::Key;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::future;
use pagable::Pagable;
use pagable::pagable_typetag;

#[derive(Clone, Dupe, PartialEq, Eq, Hash, Display, Debug, Allocative, Pagable)]
#[display("Var({})", _0)]
pub struct Var(pub Arc<String>);

#[derive(Clone, PartialEq, Eq, Hash, Debug, Allocative, Pagable)]
pub enum Unit {
    Var(Var),
    Literal(i64),
}

#[derive(PartialEq, Eq, Clone, Debug, Allocative, Pagable)]
pub enum Equation {
    Add(Vec<Unit>),
    Unit(Unit),
}

impl FromStr for Unit {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.parse::<i64>() {
            Ok(i) => Unit::Literal(i),
            Err(_) => Unit::Var(Var(Arc::new(s.into()))),
        })
    }
}

impl FromStr for Equation {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        Ok(if s.contains('+') {
            let units = s
                .split_terminator('+')
                .map(|unit| unit.parse::<Unit>())
                .collect::<Result<Vec<Unit>, _>>()
                .unwrap();

            if units.len() < 2 {
                return Err(anyhow::anyhow!("+ must have left and right"));
            }

            Equation::Add(units)
        } else {
            Equation::Unit(s.parse().unwrap())
        })
    }
}

pub fn parse_math_equations<'a>(
    math: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<Vec<(Var, Equation)>> {
    math.into_iter().map(parse_math_equation).collect()
}

pub fn parse_math_equation(math: &str) -> anyhow::Result<(Var, Equation)> {
    let Some((l, r)) = math.split_once("=") else {
        return Err(anyhow::anyhow!("= must have left and right"));
    };
    if l.is_empty() || r.is_empty() {
        return Err(anyhow::anyhow!("= must have left and right"));
    }

    Ok((Var(Arc::new(l.to_owned())), r.parse()?))
}

pub trait MathEquations {
    fn set_equation(&mut self, var: Var, equation: Equation) -> anyhow::Result<()>;
    fn set_equations(
        &mut self,
        equations: impl IntoIterator<Item = (Var, Equation)>,
    ) -> anyhow::Result<()>;
}

#[async_trait]
pub trait Math {
    async fn eval(&mut self, var: Var) -> Result<i64, Arc<anyhow::Error>>;
}

impl MathEquations for DiceTransactionUpdater {
    fn set_equation(&mut self, var: Var, equation: Equation) -> anyhow::Result<()> {
        Ok(self.changed_to(vec![(LookupVar(var), Arc::new(equation))])?)
    }
    fn set_equations(
        &mut self,
        equations: impl IntoIterator<Item = (Var, Equation)>,
    ) -> anyhow::Result<()> {
        Ok(self.changed_to(
            equations
                .into_iter()
                .map(|(var, eq)| (LookupVar(var), Arc::new(eq)))
                .collect::<Vec<_>>(),
        )?)
    }
}

#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("Eval({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
pub struct EvalVar(pub Var);
#[async_trait]
impl Key for EvalVar {
    type Value = Result<i64, Arc<anyhow::Error>>;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Result<i64, Arc<anyhow::Error>> {
        let equation = lookup_unit(ctx, &self.0).await.map_err(Arc::new)?;
        Ok(match &*equation {
            Equation::Add(adds) => resolve_units(ctx, &adds[..]).await?.iter().sum(),
            Equation::Unit(unit) => resolve_units(ctx, std::slice::from_ref(unit)).await?[0],
        })
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }
}

#[async_trait]
impl Math for DiceComputations<'_> {
    async fn eval(&mut self, var: Var) -> Result<i64, Arc<anyhow::Error>> {
        Ok(self
            .compute(&EvalVar(var))
            .await
            .map_err(|e| Arc::new(anyhow::anyhow!(e)))?
            .dupe()?)
    }
}

async fn resolve_units<'a>(
    ctx: &mut DiceComputations<'a>,
    units: &[Unit],
) -> Result<Vec<i64>, Arc<anyhow::Error>> {
    let futs = ctx.compute_many(units.iter().map(|unit| {
        DiceComputations::declare_closure(async move |ctx| match unit {
            Unit::Var(var) => ctx.eval(var.clone()).await,
            Unit::Literal(lit) => Ok(*lit),
        })
    }));

    future::join_all(futs)
        .await
        .into_iter()
        .collect::<Result<_, _>>()
}

async fn lookup_unit(ctx: &mut DiceComputations<'_>, var: &Var) -> anyhow::Result<Arc<Equation>> {
    Ok(ctx.compute(&LookupVar(var.clone())).await?.dupe())
}

#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("Lookup({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct LookupVar(Var);
impl InjectedKey for LookupVar {
    type Value = Arc<Equation>;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    fn var(name: &str) -> Var {
        Var(Arc::new(name.to_owned()))
    }

    #[test]
    fn test_parser_unit_var() {
        let eq = "a=bbab";
        assert_eq!(
            vec![(var("a"), Equation::Unit(Unit::Var(var("bbab"))))],
            parse_math_equations(vec![eq]).unwrap()
        );
    }

    #[test]
    fn test_parser_unit_int() {
        let eq = "hello world=25";
        assert_eq!(
            vec![(var("hello world"), Equation::Unit(Unit::Literal(25)))],
            parse_math_equations(vec![eq]).unwrap()
        );
    }

    #[test]
    fn test_parser_expr() {
        assert_eq!(
            vec![(
                var("a"),
                Equation::Add(vec![
                    Unit::Var(var("bd")),
                    Unit::Literal(25),
                    Unit::Var(var("b"))
                ])
            )],
            parse_math_equations(vec!["a=bd+25+b"]).unwrap()
        )
    }

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn test_parser_compound() {
        let eq = vec!["x=1", "y=2", "a=x+y", "b=a+c"];

        let x = var("x");
        let y = var("y");
        let a = var("a");
        let b = var("b");
        let c = var("c");

        assert_eq!(
            vec![
                (x.clone(), Equation::Unit(Unit::Literal(1))),
                (y.clone(), Equation::Unit(Unit::Literal(2))),
                (a.clone(), Equation::Add(vec![Unit::Var(x), Unit::Var(y)])),
                (b, Equation::Add(vec![Unit::Var(a), Unit::Var(c)]))
            ],
            parse_math_equations(eq).unwrap()
        );
    }

    #[test]
    #[should_panic(expected = "+ must have left and right")]
    fn test_parser_invalid() {
        parse_math_equations(vec!["a=a+"]).unwrap();
    }
}
