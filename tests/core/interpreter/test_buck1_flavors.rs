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

#[buck_test(env = [("BUCK2_HARD_ERROR", "false")])]
async fn test_buck1_flavors_are_mapped(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .uquery(&["deps(root//subtarget:subtarget_flavor)"])
        .await?;
    assert!(out.stdout.contains("root//lib:lib"));
    let out = buck
        .cquery(&["root//subtarget:subtarget_flavor", "--output-attribute=dep"])
        .await?;
    assert!(out.stdout.contains("root//lib:lib[shared]"));
    let out = buck
        .cquery(&["root//platform:platform_flavor", "--output-attribute=dep"])
        .await?;
    assert!(out.stdout.contains("root//lib:lib[shared]"));
    buck.build(&["root//file:hash_in_file_name"]).await?;
    Ok(())
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "only=legacy_flavor_subtarget")])]
async fn test_subtarget_flavor_hard_error(buck: &Buck) {
    buck.uquery(&["root//subtarget:subtarget_flavor"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"buck1 flavor `#shared` on `//lib:lib#shared` is deprecated.*write the subtarget as `\[shared\]`",
        );
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "true")])]
async fn test_hash_in_file_name_is_not_a_flavor(buck: &Buck) -> anyhow::Result<()> {
    // `attrs.one_of(attrs.dep(), attrs.source())` probes `data#1.json` as a label first;
    // without a `:` it is not a flavored label, so no soft error is reported.
    buck.build(&["root//file:hash_in_file_name"]).await?;
    Ok(())
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "only=platform_flavor")])]
async fn test_platform_flavor_hard_error(buck: &Buck) -> anyhow::Result<()> {
    buck.uquery(&["root//platform:platform_flavor"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"buck1 flavor `#android-arm64,shared` on `//lib:lib#android-arm64,shared` is deprecated.*write the subtarget as `\[shared\]`",
        );
    buck.uquery(&["root//lib:lib#iphoneos-arm64-tests"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"buck1 flavor `#iphoneos-arm64-tests` on `root//lib:lib#iphoneos-arm64-tests` is deprecated.*it has no effect in buck2; remove it",
        );
    // Subtarget-only flavors are a different category.
    buck.uquery(&["root//subtarget:subtarget_flavor"]).await?;
    Ok(())
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "only=legacy_flavor_unrecognized")])]
async fn test_unrecognized_flavor_hard_error(buck: &Buck) {
    buck.uquery(&["root//unrecognized:unrecognized_flavor"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"buck1 flavor `#no-such-flavor` on `//lib:lib#no-such-flavor` is deprecated.*buck2 does not support it",
        );
}
