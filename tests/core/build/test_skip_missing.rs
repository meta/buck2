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
async fn test_build_skip_missing(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&["//:existing", "//:missing", "--skip-missing-targets"])
        .await?;

    let out = std::fs::read_to_string(result.build_report()?.output_for_target("//:existing")?)?;
    assert_eq!("abcd", out.trim());
    assert!(result.stderr.contains("Skipped 1 missing targets:"));
    Ok(())
}

#[buck_test]
async fn test_build_skip_missing_fails_on_missing_package(buck: &Buck) {
    buck.build(&[
        "//:existing",
        "//bad-package:existing",
        "--skip-missing-targets",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("root//bad-package");
}
