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

// Turns `//lib/greeting` into `//lib/greeting:greeting` during coercion in
// build/bzl files. Off by default.
const ENABLE: &str = "buck2.infer_target_names=true";

#[buck_test]
async fn test_eponymous_dep_in_build_file_rejected_by_default(buck: &Buck) {
    // `//:consumer` depends on the abbreviated `//lib/greeting`. Without the
    // buckconfig this is a coercion error, matching historical behavior.
    let res = buck
        .uquery(&["deps(root//:consumer)", "--console=none", "-v0"])
        .await
        .expect_failure();
    golden(&res.stderr, "golden/eponymous_dep_rejected.golden.stderr");
}

#[buck_test]
async fn test_eponymous_dep_in_build_file_inferred_when_enabled(buck: &Buck) -> anyhow::Result<()> {
    // With the buckconfig, `//lib/greeting` is inferred to be
    // `//lib/greeting:greeting`, which is a real target, so the query succeeds.
    let res = buck
        .uquery(&["deps(root//:consumer)", "-c", ENABLE])
        .await?;
    assert!(res.stdout.contains("root//lib/greeting:greeting"));
    Ok(())
}

#[buck_test]
async fn test_eponymous_source_in_build_file_rejected_by_default(buck: &Buck) {
    // `attrs.source()` takes either a path or a label, so it coerces separately
    // from `attrs.dep()`. Without the buckconfig, `//lib/greeting` fails as both
    // a label without an explicit target name and as a non-relative path.
    buck.uquery(&["deps(root//source:src_consumer)", "--console=none", "-v0"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"Couldn't coerce `//lib/greeting` as a source");
}

#[buck_test]
async fn test_eponymous_source_in_build_file_inferred_when_enabled(
    buck: &Buck,
) -> anyhow::Result<()> {
    // With the buckconfig, the source attr coerces to the eponymous label rather
    // than being (mis)treated as a path.
    let res = buck
        .uquery(&["deps(root//source:src_consumer)", "-c", ENABLE])
        .await?;
    assert!(res.stdout.contains("root//lib/greeting:greeting"));
    Ok(())
}

#[buck_test]
async fn test_eponymous_attr_default_in_bzl_rejected_by_default(buck: &Buck) {
    // The `//bzl_default` rule declares `attrs.dep(default = "//lib/greeting")`.
    // The default is coerced while evaluating the .bzl module, so it errors
    // without the buckconfig.
    let res = buck
        .uquery(&["root//bzl_default:", "--console=none", "-v0"])
        .await
        .expect_failure();
    golden(
        &res.stderr,
        "golden/eponymous_attr_default_rejected.golden.stderr",
    );
}

#[buck_test]
async fn test_eponymous_attr_default_in_bzl_inferred_when_enabled(
    buck: &Buck,
) -> anyhow::Result<()> {
    // With the buckconfig, the eponymous attr default resolves and shows up as a
    // dependency of the target.
    let res = buck
        .uquery(&["deps(root//bzl_default:target)", "-c", ENABLE])
        .await?;
    assert!(res.stdout.contains("root//lib/greeting:greeting"));
    Ok(())
}
