/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;

#[buck_test]
async fn test_summary_command(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:my_rule"]).await?;
    let out = buck.log(&["summary"]).await?;

    assert!(out.stdout.contains("Showing summary from:"));
    assert!(out.stdout.contains("Targets Analyzed: 1"));
    Ok(())
}
