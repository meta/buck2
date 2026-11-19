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

#[buck_test(data_dir = "deprecated_correct")]
async fn test_owner_without_universe_correct(buck: &Buck) -> anyhow::Result<()> {
    // TODO(nga): there should be a warning.
    let result = buck.cquery(&["owner(bin.sh)"]).await?;
    assert_eq!("", result.stdout);
    assert!(
        result
            .stderr
            .contains("Query has no target literals and `--target-universe` is not specified")
    );
    Ok(())
}

#[buck_test(data_dir = "deprecated_correct")]
async fn test_owner_with_auto_universe_correct(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&["deps(//:test) intersect owner(bin.sh)"])
        .await?;
    // Drop configuration.
    let targets: Vec<&str> = result
        .stdout
        .lines()
        .map(|t| t.split_whitespace().next().unwrap_or_default())
        .collect();
    assert_eq!(vec!["root//:bin"], targets);
    Ok(())
}
