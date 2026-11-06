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

#[buck_test(
    setup_eden = true,
    // buck-out is not redirected in the subproject
    allow_soft_errors = true,
    // We don't run buck outside the subproject
    skip_final_kill = true,
)]
async fn test_mismatched_root(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//..."]).rel_cwd("subdir").await?;
    buck.kill().rel_cwd("subdir").await?;
    Ok(())
}
