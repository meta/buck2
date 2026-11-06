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
use buck2_e2e::utils::replace_hash;

#[buck_test]
async fn test_ctargets_skip_missing_targets(buck: &Buck) -> anyhow::Result<()> {
    buck.ctargets(&[
        "root//:existing",
        "root//:nonexistent",
        "--target-platforms=root//:p",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Unknown target `nonexistent` from package");

    let result = buck
        .ctargets(&[
            "root//:existing",
            "root//:nonexistent",
            "--target-platforms=root//:p",
            "--skip-missing-targets",
        ])
        .await?;
    let lines: Vec<&str> = result.stdout.lines().collect();
    let [line] = lines.as_slice() else {
        panic!("expected one line: {lines:?}");
    };
    let line = replace_hash(line);
    assert_eq!(line, "root//:existing (root//:p#<HASH>)");

    assert!(result.stderr.contains("Skipped 1 missing targets:"));
    assert!(result.stderr.contains("root//:nonexistent"));
    Ok(())
}
