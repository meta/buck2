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
async fn test_ctargets_incompatible(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            // This one will be omitted from the output because it is not compatible.
            "root//:triangle",
            // This one will be output.
            "root//:square",
            "--target-platforms=root//:rectangular",
        ])
        .await?;
    let stdout = replace_hash(&result.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    let [line] = lines.as_slice() else {
        panic!("expected one line: {lines:?}");
    };
    assert_eq!(*line, "root//:square (root//:rectangular#<HASH>)");

    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    assert!(
        result
            .stderr
            .contains("root//:triangle (root//:rectangular#")
    );
    Ok(())
}
