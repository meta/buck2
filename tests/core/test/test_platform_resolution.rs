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

#[buck_test(env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")])]
async fn test_platform_resolution(buck: &mut Buck) -> anyhow::Result<()> {
    // An empty test executor forces the internal test executor to be used.
    buck.set_env("BUCK2_TPX", "");
    // Setup is such that test target is incompatible with testee's default
    // target platform.
    buck.test(&[":my_rule"]).await?;
    Ok(())
}
