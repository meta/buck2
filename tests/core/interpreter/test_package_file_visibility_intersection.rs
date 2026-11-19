/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;

fn set_default_intersection(buck: &Buck, mode: &str) -> std::io::Result<()> {
    std::fs::write(
        buck.cwd().join(".bucksettings.toml"),
        format!("[package_visibility]\ndefault_intersection = \"{mode}\"\n"),
    )
}

#[buck_test]
async fn test_optin_inside_consumer_can_depend_on_public_target(buck: &Buck) -> anyhow::Result<()> {
    // PUBLIC clipped to the intersection; inside consumer matches.
    buck.ctargets(&["root//intersect/inside_consumer:c"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_optin_clips_public_target_for_outside_consumer(buck: &Buck) {
    // PUBLIC silently clipped (not rejected); outside consumer fails.
    // Locks the diagnostic: error mentions visibility attr and the restriction.
    let result = buck
        .ctargets(&["root//outside_consumer:c"])
        .await
        .expect_failure();
    result.assert_stderr_matches(
        r"is not visible to.*visibility = .*Restricted to.*by an ancestor PACKAGE's visibility",
    );
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/test_optin_clips_public_target_for_outside_consumer.golden.txt",
    );
}

#[buck_test]
async fn test_optin_intersection_blocks_visibility_leak_outside(buck: &Buck) {
    // Target's own `visibility` lists `leak_destination/...`; the
    // intersection blocks it.
    // Differs from `enforce_strict_visibility` which would allow this leak.
    let result = buck
        .ctargets(&["root//leak_destination/consumer:c"])
        .await
        .expect_failure();
    result.assert_stderr_matches(
        r"is not visible to.*Restricted to.*by an ancestor PACKAGE's visibility",
    );
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/test_optin_intersection_blocks_visibility_leak_outside.golden.txt",
    );
}

#[buck_test]
async fn test_optin_target_own_visibility_match_passes(buck: &Buck) -> anyhow::Result<()> {
    // Consumer matches both visibility attr and intersection.
    buck.ctargets(&["root//intersect/sub_b/consumer:c"]).await?;
    Ok(())
}

#[buck_test]
async fn test_inherit_true_child_can_still_tighten(buck: &Buck) -> anyhow::Result<()> {
    // Regression: with `inherit=True`, the child contributes its EXPLICIT
    // `visibility=B` to the intersection (not `parent.visibility ∪ B`), so a
    // tighter child intersection is not silently absorbed into the parent's.
    let result = buck
        .ctargets(&["root//inherit_test/other/consumer:c"])
        .await
        .expect_failure();
    result.assert_stderr_matches(r"is not visible to");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/test_inherit_true_child_can_still_tighten.golden.txt",
    );
    buck.ctargets(&["root//inherit_test/restricted_child/inside/consumer:c"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_package_with_omitted_visibility_does_not_empty_intersection(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Regression: `package(inherit=True, within_view=[...])` (no `visibility=`)
    // combined with `enforce_visibility_intersection()` must NOT contribute an
    // empty list to the intersection. Before the fix, the omitted `visibility=`
    // defaulted to `[]` and was treated as an explicit empty contribution,
    // narrowing the intersection down to the empty set and blocking all
    // consumers.
    buck.ctargets(&["root//inherit_test/no_vis_child/inside_consumer:c"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_optin_preserves_parent_within_view(buck: &Buck) -> anyhow::Result<()> {
    // Regression: opt-in must not widen the inherited `within_view` to PUBLIC
    // (would happen if implementation routed through `package(...)`).
    buck.ctargets(&["root//within_view_preserve/child/ok_dep:c"])
        .await?;
    let result = buck
        .ctargets(&["root//within_view_preserve/child_bad/bad_dep:c"])
        .await
        .expect_failure();
    result.assert_stderr_matches(r"within_view");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/test_optin_preserves_parent_within_view.golden.txt",
    );
    Ok(())
}

#[buck_test]
async fn test_call_from_bzl_is_rejected(buck: &Buck) {
    let result = buck
        .ctargets(&["root//indirect_call/leaf:t"])
        .await
        .expect_failure();
    result.assert_stderr_matches(
        r"`enforce_visibility_intersection\(\)` can only be called from a `PACKAGE` file",
    );
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/test_call_from_bzl_is_rejected.golden.txt",
    );
}

#[buck_test]
async fn test_calling_twice_in_one_package_is_rejected(buck: &Buck) {
    // A second `enforce_visibility_intersection()` in the same PACKAGE file fails.
    let result = buck
        .ctargets(&["root//at_most_once/leaf:t"])
        .await
        .expect_failure();
    result.assert_stderr_matches(
        r"`enforce_visibility_intersection\(\)` function can be used at most once per `PACKAGE` file",
    );
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/test_calling_twice_in_one_package_is_rejected.golden.txt",
    );
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "false")])]
async fn test_audit_mode_still_enforces_marker_boundary(buck: &Buck) -> anyhow::Result<()> {
    set_default_intersection(buck, "audit")?;
    for consumer in [
        "root//outside_consumer:c",
        "root//leak_destination/consumer:c",
    ] {
        buck.ctargets(&[consumer])
            .await
            .expect_failure()
            .assert_stderr_matches(
                r"is not visible to.*Restricted to.*by an ancestor PACKAGE's visibility",
            );
    }
    Ok(())
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "false")])]
async fn test_audit_mode_still_enforces_own_visibility(buck: &Buck) -> anyhow::Result<()> {
    set_default_intersection(buck, "audit")?;
    buck.ctargets(&["root//intersect/sub_a/consumer:c"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"is not visible to");
    Ok(())
}
