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
async fn test_skip_targets_with_duplicate_names_without_flag(buck: &Buck) {
    buck.targets(&["//..."])
        .await
        .expect_failure()
        .assert_stderr_matches("Attempted to register target prelude//:aa twice");
}

#[buck_test]
async fn test_skip_targets_with_duplicate_names_with_flag(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&["//...", "--skip-targets-with-duplicate-names"])
        .await?;
    assert_eq!(
        vec!["prelude//:aa", "prelude//:bb"],
        result.stdout.lines().collect::<Vec<_>>()
    );
    assert!(
        result
            .stderr
            .contains("Attempted to register target prelude//:aa twice")
    );
    assert!(
        result
            .stderr
            .contains("Attempted to register target prelude//:bb twice")
    );
    Ok(())
}
