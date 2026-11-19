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

#[buck_test(skip_for_os = ["windows"])]
async fn test_symlink_to_parent_bug(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&["//:whistle", "--prefer-local", "--no-remote-cache"])
        .await?;
    let out = result.build_report()?.output_for_target("//:whistle")?;
    assert!(out.to_string_lossy().ends_with("/whistle"));
    // Check the link was actually materialized.
    assert!(out.is_symlink());
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_symlink_to_self(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&["//:flute", "--prefer-local", "--no-remote-cache"])
        .await?;
    let out = result.build_report()?.output_for_target("//:flute")?;
    assert!(out.to_string_lossy().ends_with("/flute"));
    // Check the link was actually materialized.
    assert!(out.is_symlink());
    Ok(())
}

#[buck_test]
async fn test_noop(_buck: &Buck) {}
