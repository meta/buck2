/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use dice::DetectCycles;
use dice::Dice;
use dice_examples::math_computation::Equation;
use dice_examples::math_computation::Math;
use dice_examples::math_computation::MathEquations;
use dice_examples::math_computation::Unit;
use dice_examples::math_computation::Var;
use dice_examples::math_computation::parse_math_equation;
use dice_examples::math_computation::parse_math_equations;
use dupe::Dupe;

fn var(name: &str) -> Var {
    Var(Arc::new(name.to_owned()))
}

#[tokio::test]
async fn test_literal() -> Result<(), Arc<anyhow::Error>> {
    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    let mut ctx = dice.updater(root);
    let (var, eq) = parse_math_equation("a=5").unwrap();
    ctx.set_equation(var.dupe(), eq)?;
    let ctx = ctx.commit().await;

    assert_eq!(5, ctx.ctx().eval(var).await?);

    Ok(())
}

#[tokio::test]
async fn test_var() -> Result<(), Arc<anyhow::Error>> {
    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    let mut ctx = dice.updater(root);

    let eqs = parse_math_equations(vec!["b=a", "a=3"]).unwrap();

    ctx.set_equations(eqs)?;
    let ctx = ctx.commit().await;

    assert_eq!(3, ctx.ctx().eval(var("b")).await?);
    Ok(())
}

#[tokio::test]
async fn test_compound() -> Result<(), Arc<anyhow::Error>> {
    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    let mut ctx = dice.updater(root);

    let eq = vec!["x=1", "y=2", "a=x+y", "b=a+a"];
    let eq = parse_math_equations(eq).unwrap();

    ctx.set_equations(eq)?;
    let ctx = ctx.commit().await;

    assert_eq!(3, ctx.ctx().eval(var("a")).await?);
    assert_eq!(6, ctx.ctx().eval(var("b")).await?);

    Ok(())
}

#[tokio::test]
async fn test_changed_eq() -> Result<(), Arc<anyhow::Error>> {
    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    let mut ctx = dice.updater(root);

    let eq = vec!["x=1", "y=2", "a=x+y", "b=a+a"];
    let eq = parse_math_equations(eq).unwrap();

    ctx.set_equations(eq)?;
    let ctx = ctx.commit().await;

    assert_eq!(6, ctx.ctx().eval(var("b")).await?);

    let mut ctx = dice.updater(root);
    ctx.set_equation(var("a"), Equation::Unit(Unit::Literal(4)))?;
    let ctx = ctx.commit().await;

    assert_eq!(8, ctx.ctx().eval(var("b")).await?);

    Ok(())
}
