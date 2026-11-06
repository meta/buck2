/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

use buck2_e2e::prelude::*;

fn append_buckconfig(buck: &Buck, text: &str) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(text.as_bytes())?;
    Ok(())
}

/// An empty `package_includes` value and a trailing comma are not parse errors.
async fn empty_items_are_skipped(buck: &Buck, value: &str) -> anyhow::Result<()> {
    append_buckconfig(buck, &format!("[buildfile]\npackage_includes = {value}\n"))?;
    let result = buck.targets(&["//:"]).await?;
    assert_eq!(result.stdout.trim(), "root//:t");
    Ok(())
}

#[buck_test]
async fn test_empty_items_are_skipped_case(buck: &Buck) -> anyhow::Result<()> {
    empty_items_are_skipped(buck, "").await
}

#[buck_test]
async fn test_empty_items_are_skipped_src_defs_bzl_my_rule(buck: &Buck) -> anyhow::Result<()> {
    empty_items_are_skipped(buck, "src=>//:defs.bzl::my_rule,").await
}

/// `src =>` applies to the directory `src`, not to a directory named `src `.
#[buck_test]
async fn test_blanks_around_the_arrow_are_trimmed(buck: &Buck) -> anyhow::Result<()> {
    append_buckconfig(
        buck,
        "[buildfile]\npackage_includes = src =>//:defs.bzl::alias = my_rule\n",
    )?;
    let result = buck.targets(&["//src:"]).await?;
    assert_eq!(result.stdout.trim(), "root//src:included");
    Ok(())
}
