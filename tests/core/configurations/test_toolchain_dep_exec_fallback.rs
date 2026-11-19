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
async fn test_toolchain_dep_with_resolved_candidate(buck: &Buck) -> anyhow::Result<()> {
    // The toolchain dep finds the parent's exec platform among the candidates.
    let result = buck.cquery(&["deps(root//:resolves)"]).await?;
    assert!(result.stdout.contains("root//:tc (root//:p#"));
    Ok(())
}

#[buck_test]
async fn test_toolchain_dep_with_fallback_platform(buck: &Buck) -> anyhow::Result<()> {
    // Every candidate is skipped, so resolution returns the fallback `ExecutionPlatformInfo`
    // (sunos); the toolchain dep inherits it.
    let result = buck
        .cquery(&[
            "deps(root//:falls_back)",
            "-c",
            "build.execution_platforms=root//execution_platforms:execution_platforms_platform_fallback",
        ])
        .await?;
    assert!(result.stdout.contains("root//:tc (root//:p#"));
    assert!(result.stdout.contains("sunos"));
    Ok(())
}

#[buck_test]
async fn test_toolchain_dep_with_unspecified_fallback(buck: &Buck) -> anyhow::Result<()> {
    // With `fallback` unset the parent proceeds with the unspecified exec platform, and so
    // does its toolchain dep.
    let result = buck.cquery(&["deps(root//:falls_back)"]).await?;
    assert!(result.stdout.contains("root//:tc (root//:p#"));
    assert!(result.stdout.contains("unspecified_exec"));
    Ok(())
}
