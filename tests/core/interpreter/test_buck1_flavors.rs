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
async fn test_subtarget_flavor_is_an_error(buck: &Buck) -> anyhow::Result<()> {
    buck.uquery(&["root//subtarget:subtarget_flavor"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"buck1 flavor `#shared` on `//lib:lib#shared` is not supported.*write the subtarget as `\[shared\]`",
        );
    // The same label spelled as a subtarget works.
    let out = buck.audit(&["providers", "root//lib:lib[shared]"]).await?;
    assert!(out.stdout.contains("root//lib:lib[shared]"));
    // A string that is not a label still coerces as a file.
    buck.build(&["root//file:hash_in_file_name"]).await?;
    Ok(())
}

#[buck_test]
async fn test_platform_flavor_is_an_error(buck: &Buck) {
    buck.uquery(&["root//platform:platform_flavor"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"buck1 flavor `#android-arm64,shared` on `//lib:lib#android-arm64,shared` is not supported.*write the subtarget as `\[shared\]`",
        );
    buck.uquery(&["root//lib:lib#iphoneos-arm64-tests"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"buck1 flavor `#iphoneos-arm64-tests` on `root//lib:lib#iphoneos-arm64-tests` is not supported.*it has no effect in buck2; remove it",
        );
}

#[buck_test]
async fn test_unrecognized_flavor_is_an_error(buck: &Buck) {
    buck.uquery(&["root//unrecognized:unrecognized_flavor"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"buck1 flavor `#no-such-flavor` on `//lib:lib#no-such-flavor` is not supported.*buck2 has no equivalent for it",
        );
}

#[buck_test]
async fn test_one_of_does_not_swallow_a_flavored_label(buck: &Buck) -> anyhow::Result<()> {
    // `attrs.one_of(attrs.dep(), attrs.string())` would otherwise accept the label as text.
    buck.uquery(&["root//one_of:flavored_dep_or_string"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"buck1 flavor `#shared` on `//lib:lib#shared` is not supported");
    // Text that only happens to contain a `:` and a `#` is still a string.
    buck.build(&["root//text:shell_command"]).await?;
    Ok(())
}
