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
use serde_json::Value;
use serde_json::json;

// `within_view` is enforced while the build file is evaluated, so `uquery` is
// enough to exercise it (no configuration needed).

// Inside an opted-in subtree every refusal names both the target's own list and
// the cap; outside one, the message is the ordinary `within_view` error.
const CAP_REGEX: &str = concat!(
    r"Target's effective `within_view` does not allow dependency",
    r".*Capped to.*enforce_within_view_intersection",
);
const OWN_LIST_REGEX: &str = r"Target's `within_view` attribute does not allow dependency";

/// Expects `uquery` of `target` to fail with `stderr_regex` and its sanitized stderr to match the
/// golden file `golden_name`.
async fn expect_refusal(buck: &Buck, target: &str, stderr_regex: &str, golden_name: &str) {
    let result = buck.uquery(&[target]).await.expect_failure();
    result.assert_stderr_matches(stderr_regex);
    golden(
        &sanitize_stderr(&result.stderr),
        &format!("golden/{golden_name}.golden.txt"),
    );
}

#[buck_test]
async fn test_optin_inside_dep_is_allowed(buck: &Buck) -> anyhow::Result<()> {
    buck.uquery(&["root//intersect/inside_dep:c"]).await?;
    Ok(())
}

#[buck_test]
async fn test_optin_cap_refuses_public_per_target_within_view(buck: &Buck) {
    // A per-target `within_view = ["PUBLIC"]` replaces the PACKAGE default but
    // cannot escape the cap. Locks the diagnostic: the target's own list and
    // the cap are both named.
    expect_refusal(
        buck,
        "root//intersect/public_target:c",
        CAP_REGEX,
        "test_optin_cap_refuses_public_per_target_within_view",
    )
    .await;
}

#[buck_test]
async fn test_optin_cap_refuses_per_target_within_view_listing_outside_dep(buck: &Buck) {
    // The target's own `within_view` explicitly names `outside/...`; the cap
    // still refuses it.
    expect_refusal(
        buck,
        "root//intersect/leaky_target:c",
        CAP_REGEX,
        "test_optin_cap_refuses_per_target_within_view_listing_outside_dep",
    )
    .await;
}

#[buck_test]
async fn test_per_target_within_view_narrower_than_cap_wins(buck: &Buck) -> anyhow::Result<()> {
    // The target's own list is what refuses the dep here (the dep is inside the
    // cap). Inside an opted-in subtree the message still names the cap next to
    // the target's own list, as `enforce_visibility_intersection()` does.
    expect_refusal(
        buck,
        "root//intersect/narrow_target:c",
        CAP_REGEX,
        "test_per_target_within_view_narrower_than_cap_wins",
    )
    .await;
    buck.uquery(&["root//intersect/narrow_target_ok:c"]).await?;
    Ok(())
}

#[buck_test]
async fn test_nested_package_public_within_view_is_still_capped(buck: &Buck) -> anyhow::Result<()> {
    // `package(inherit = False, within_view = ["PUBLIC"])` in a descendant
    // replaces the default but not the cap.
    buck.uquery(&["root//intersect/intermediate/leaf_ok:c"])
        .await?;
    expect_refusal(
        buck,
        "root//intersect/intermediate/leaf_bad:c",
        CAP_REGEX,
        "test_nested_package_public_within_view_is_still_capped",
    )
    .await;
    Ok(())
}

#[buck_test]
async fn test_intersect_grandparent_parent_child(buck: &Buck) -> anyhow::Result<()> {
    // Three opted-in levels: the effective cap is the AND of all three lists.
    buck.uquery(&["root//intersect/child/grandchild/ok:c"])
        .await?;
    expect_refusal(
        buck,
        "root//intersect/child/grandchild/bad:c",
        r"Capped to .* AND .* AND .* by `enforce_within_view_intersection\(\)`",
        "test_intersect_grandparent_parent_child",
    )
    .await;
    Ok(())
}

#[buck_test]
async fn test_optin_only_package_keeps_parent_default_and_cap(buck: &Buck) -> anyhow::Result<()> {
    // A PACKAGE that only calls `enforce_within_view_intersection()` (no
    // `package()`) neither widens the inherited `within_view` default nor
    // changes the cap. The default alone refuses the outside dep; the message
    // names the (inherited) cap as well.
    buck.uquery(&["root//intersect/optin_only/ok:c"]).await?;
    expect_refusal(
        buck,
        "root//intersect/optin_only/bad:c",
        CAP_REGEX,
        "test_optin_only_package_keeps_parent_default_and_cap",
    )
    .await;
    Ok(())
}

#[buck_test]
async fn test_optin_without_within_view_propagates_parent_cap(buck: &Buck) -> anyhow::Result<()> {
    // `package(inherit = True, visibility = [...])` (no `within_view=`) with
    // `enforce_within_view_intersection()` contributes nothing to the cap. The
    // parent's cap still applies -- and catches the PUBLIC default that
    // `inherit = True` with an omitted `within_view` produces.
    buck.uquery(&["root//inherit_test/no_wv_child/inside_dep:c"])
        .await?;
    expect_refusal(
        buck,
        "root//inherit_test/no_wv_child/outside_dep:c",
        CAP_REGEX,
        "test_optin_without_within_view_propagates_parent_cap",
    )
    .await;
    Ok(())
}

#[buck_test]
async fn test_inherit_true_child_can_still_tighten_cap(buck: &Buck) -> anyhow::Result<()> {
    // With `inherit = True`, the child contributes its EXPLICIT `within_view`
    // to the cap (not `parent ∪ child`), so the cap tightens even though the
    // default `within_view` below is the union.
    buck.uquery(&["root//inherit_test/restricted_child/inside:c"])
        .await?;
    expect_refusal(
        buck,
        "root//inherit_test/restricted_child/escaping:c",
        CAP_REGEX,
        "test_inherit_true_child_can_still_tighten_cap",
    )
    .await;
    Ok(())
}

#[buck_test]
async fn test_public_is_identity(buck: &Buck) -> anyhow::Result<()> {
    buck.uquery(&["root//public_identity/consumer:c"]).await?;
    let stdout = buck
        .audit(&["package-values", "root//public_identity"])
        .await?
        .stdout;
    let result: Value = serde_json::from_str(&stdout)?;
    assert_eq!(
        result["root//public_identity"]["within_view_cap"],
        json!(["PUBLIC"])
    );
    Ok(())
}

#[buck_test]
async fn test_same_package_deps_are_exempt(buck: &Buck) -> anyhow::Result<()> {
    buck.uquery(&["root//same_package:c"]).await?;
    Ok(())
}

#[buck_test]
async fn test_select_arm_is_capped(buck: &Buck) {
    expect_refusal(
        buck,
        "root//select_arm:c",
        CAP_REGEX,
        "test_select_arm_is_capped",
    )
    .await;
}

#[buck_test]
async fn test_exec_and_toolchain_deps_are_capped(buck: &Buck) {
    // `within_view` covers every dep-typed attribute, exec and toolchain deps
    // included; so does the cap.
    expect_refusal(
        buck,
        "root//exec_dep:c",
        CAP_REGEX,
        "test_exec_dep_is_capped",
    )
    .await;
    expect_refusal(
        buck,
        "root//exec_dep/toolchain:c",
        CAP_REGEX,
        "test_toolchain_dep_is_capped",
    )
    .await;
}

#[buck_test]
async fn test_outside_optin_message_is_unchanged(buck: &Buck) {
    // No ancestor opted in: the ordinary `within_view` error, without any
    // mention of a cap.
    let result = buck
        .uquery(&["root//no_optin/bad:c"])
        .await
        .expect_failure();
    result.assert_stderr_matches(OWN_LIST_REGEX);
    assert!(!result.stderr.contains("Capped to"));
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/test_outside_optin_message_is_unchanged.golden.txt",
    );
}

#[buck_test]
async fn test_audit_package_values_shows_within_view_cap(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck
        .audit(&[
            "package-values",
            "root//intersect",
            "root//intersect/intermediate",
            "root//intersect/child/grandchild",
        ])
        .await?
        .stdout;
    let result: Value = serde_json::from_str(&stdout)?;
    assert_eq!(
        result["root//intersect"]["within_view_cap"],
        json!(["root//intersect/..."])
    );
    // A non-opted-in descendant inherits the cap unchanged, while its own
    // `within_view` is what its `package()` call said.
    assert_eq!(
        result["root//intersect/intermediate"]["within_view_cap"],
        json!(["root//intersect/..."])
    );
    assert_eq!(
        result["root//intersect/intermediate"]["within_view"],
        json!(["PUBLIC"])
    );
    assert_eq!(
        result["root//intersect/child/grandchild"]["within_view_cap"],
        json!({
            "intersection": [
                ["root//intersect/..."],
                ["root//intersect/child/...", "root//outside/..."],
                [
                    "root//intersect/child/grandchild/...",
                    "root//intersect/child/lib:",
                    "root//outside/...",
                ],
            ]
        })
    );
    Ok(())
}

#[buck_test]
async fn test_call_from_bzl_is_rejected(buck: &Buck) {
    expect_refusal(
        buck,
        "root//indirect_call/leaf:t",
        r"`enforce_within_view_intersection\(\)` can only be called from a `PACKAGE` file",
        "test_call_from_bzl_is_rejected",
    )
    .await;
}

#[buck_test]
async fn test_calling_twice_in_one_package_is_rejected(buck: &Buck) {
    expect_refusal(
        buck,
        "root//at_most_once/leaf:t",
        r"`enforce_within_view_intersection\(\)` function can be used at most once per `PACKAGE` file",
        "test_calling_twice_in_one_package_is_rejected",
    )
    .await;
}
