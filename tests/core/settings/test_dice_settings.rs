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

// A `dice` section reaches computations through DICE: changing it between
// commands re-evaluates what depended on it, in the same daemon. The
// Starlark call stack limit is the vehicle.
#[buck_test]
async fn test_dice_setting_changes_apply_without_restart(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":"]).await?;

    buck.targets(&[":", "--setting", "starlark.max_callstack_size=10"])
        .await
        .expect_failure()
        .assert_stderr_matches("Starlark call stack overflow");

    let result = buck
        .targets(&[":", "--setting", "starlark.max_callstack_size=100"])
        .await?;
    assert!(!result.stderr.contains("restarting daemon"));

    // Back to the default, still the same daemon.
    let result = buck.targets(&[":"]).await?;
    assert!(!result.stderr.contains("restarting daemon"));
    Ok(())
}
