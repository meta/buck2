/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! What the modifier tests share.

use anyhow::Context;
use buck2_e2e::prelude::*;

/// The configuration of the one target `ctargets <args>` prints, as `audit configurations` prints it.
pub async fn get_cfg(buck: &Buck, args: &[&str]) -> anyhow::Result<String> {
    let result = buck.ctargets(args).await?;

    // Assuming ctargets output is `target (cfg)`
    let cfg = result
        .stdout
        .split_whitespace()
        .nth(1)
        .context("No configuration in the `ctargets` output")?
        .trim_matches(['(', ')']);

    let result = buck.audit_configurations(&[cfg]).await?;
    Ok(result.stdout)
}
