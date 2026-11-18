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
async fn test_configuration_transition_attr_split_cquery(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.cquery(&["deps(root//:bb)"]).await?;
    // Check both transitioned deps are present.
    assert!(result.stdout.contains("root//:code (arm64#"));
    assert!(result.stdout.contains("root//:code (arm32#"));
    Ok(())
}

#[buck_test]
async fn test_configuration_transition_attr_split_build(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:bb"]).await?;
    // Rule implementations do the assertions.
    Ok(())
}
