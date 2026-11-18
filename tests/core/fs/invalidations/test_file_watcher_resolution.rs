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
async fn test_changing_cell_location_bug(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["foo//:", "bar//:"]).await?;

    // Switch the location of the 2 cells
    std::fs::write(
        buck.cwd().join(".buckconfig"),
        "[cells]\nfoo=bar\nbar=foo\nroot=.\nprelude=.\n",
    )?;

    // Make sure buck picks up the `CellResolver` updates
    buck.targets(&["foo//:", "bar//:"]).await?;

    std::fs::write(
        buck.cwd().join("foo").join("TARGETS.fixture"),
        "fail('error')",
    )?;

    buck.targets(&["foo//:", "bar//:"])
        .await
        .expect_failure()
        .assert_stderr_matches("fail: error");
    Ok(())
}
