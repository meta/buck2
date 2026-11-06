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

// needed to avoid failure on missing buck2-tpx in buck-out
#[buck_test(env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")])]
async fn test_test_skip_incompatible_targets(buck: &mut Buck) -> anyhow::Result<()> {
    let target_a = "root//:compatible-with-A";
    let target_b = "root//:compatible-with-B";
    let platform_a = "root//:platA";
    let target_platforms = format!("--target-platforms={platform_a}");

    // An empty test executor forces the internal test executor to be used.
    buck.set_env("BUCK2_TPX", "");

    buck.test(&[target_a, target_b, &target_platforms])
        .await
        .expect_failure()
        .assert_stderr_matches(&format!(
            r"{target_b}\s*is incompatible with {platform_a}#.*$"
        ));

    let result = buck
        .test(&[
            target_a,
            target_b,
            &target_platforms,
            "--skip-incompatible-targets",
        ])
        .await?;
    assert!(result.stderr.contains(target_a));
    assert!(!result.stderr.contains(target_b));

    result.assert_exit_code(ExitCode::Success);
    Ok(())
}
