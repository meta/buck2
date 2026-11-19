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
async fn test_configuration_transition_attr(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.cquery(&["deps(root//:the-test)"]).await?;
    // Default configuration is iphoneos and it should be transitioned to watchos
    assert!(result.stdout.contains(":watchos_resource"));
    assert!(!result.stdout.contains(":default_resource"));
    Ok(())
}
