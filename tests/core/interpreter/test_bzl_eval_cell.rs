/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `other//defs.bzl` is loaded by a build file in its own cell and by one in the root cell.
//! Each build file records what the top level of `defs.bzl` saw of the cell it was evaluated
//! for, and what a function of `defs.bzl` sees when the build file calls it.

use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;

const OWN_CELL: &str = "buck2.eval_bzl_in_own_cell=true";

const TARGETS: &str =
    "set(root//:top_level root//:in_function other//:top_level other//:in_function)";

async fn query(buck: &Buck, config: &[&str]) -> anyhow::Result<BuckOutput> {
    let mut args = vec![
        "--json",
        "--output-attribute=^buck\\.type$",
        "--output-attribute=^cells$",
    ];
    args.extend(config);
    args.push(TARGETS);
    Ok(buck.uquery(&args).await?)
}

fn evaluations(output: &BuckOutput) -> usize {
    output.stderr.matches("evaluating defs.bzl").count()
}

/// By default `defs.bzl` is evaluated for the cell of each build file that loads it.
#[buck_test]
async fn test_bzl_evaluated_for_each_loading_cell(buck: &Buck) -> anyhow::Result<()> {
    let output = query(buck, &[]).await?;
    assert_eq!(2, evaluations(&output));
    golden(&output.stdout, "golden/loader_cell.golden.json");
    Ok(())
}

/// With the buckconfig, `defs.bzl` is evaluated once, for the cell it is in. Functions still
/// see the cell of the build file that calls them.
#[buck_test]
async fn test_bzl_evaluated_once_for_own_cell(buck: &Buck) -> anyhow::Result<()> {
    let output = query(buck, &["-c", OWN_CELL]).await?;
    assert_eq!(1, evaluations(&output));
    golden(&output.stdout, "golden/own_cell.golden.json");
    Ok(())
}

/// A command that names a `bzl` file follows the buckconfig like a `load()` does.
#[buck_test]
async fn test_audit_starlark_module(buck: &Buck) -> anyhow::Result<()> {
    let module = ["starlark", "module", "other//:defs.bzl"];

    let output = buck.audit(&module).await?;
    assert!(output.stdout.starts_with("other//defs.bzl@root\n"));

    // In a file, because `audit starlark module` does not take `-c`.
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "[buck2]\neval_bzl_in_own_cell = true\n",
    )?;
    let output = buck.audit(&module).await?;
    assert!(output.stdout.starts_with("other//defs.bzl\n"));
    Ok(())
}
