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

// `project.ignore` is read when the daemon starts. A command that asks for a
// different one is refused rather than run with the daemon's. The data sets it
// behind a `<file:...>` include, as fbsource does, which not every config parse
// follows.

#[buck_test]
async fn test_override_is_refused(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":"]).await?;
    buck.targets(&[":", "-c", "project.ignore=other"])
        .await
        .expect_failure()
        .assert_stderr_matches("`project.ignore` cannot be set with `--config`");
    Ok(())
}

#[buck_test]
async fn test_change_needs_a_restart(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":"]).await?;

    let ignores = buck.cwd().join("ignores.bcfg");
    std::fs::write(
        &ignores,
        std::fs::read_to_string(&ignores)?.replace("ignore = ignored", "ignore = other"),
    )?;
    buck.targets(&[":"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "`project.ignore` of cell `root` differs from what the daemon read when it started",
        );

    buck.kill().await?;
    buck.targets(&[":"]).await?;
    Ok(())
}
