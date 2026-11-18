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

fn set_default_intersection(buck: &Buck, mode: &str) -> std::io::Result<()> {
    let settings = match mode {
        "audit" => "[package_visibility]\ndefault_intersection = \"audit\"\n",
        "enforce" => "[package_visibility]\ndefault_intersection = \"enforce\"\n",
        _ => panic!("No settings for mode `{mode}`"),
    };
    // Read at daemon startup, so this must run before the first command.
    std::fs::write(buck.cwd().join(".bucksettings.toml"), settings)
}

#[buck_test]
async fn test_off_mode_boundary_is_dormant(buck: &Buck) -> anyhow::Result<()> {
    buck.ctargets(&["root//outside:uses_legacy", "root//outside:uses_strict"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_enforce_mode_exempt_package_stays_visible(buck: &Buck) -> anyhow::Result<()> {
    set_default_intersection(buck, "enforce")?;
    buck.ctargets(&["root//outside:uses_legacy"]).await?;
    buck.ctargets(&["root//outside:uses_strict"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"`root//boundary/strict:t` is not visible to");
    Ok(())
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "false")])]
async fn test_audit_mode_reports_instead_of_failing(buck: &Buck) -> anyhow::Result<()> {
    set_default_intersection(buck, "audit")?;
    buck.ctargets(&["root//outside:uses_legacy"]).await?;
    buck.ctargets(&["root//outside:uses_strict"]).await?;
    // Recorded as a deprecation, which is not shown on the console
    let log = buck.log(&["show"]).await?;
    assert!(log.stdout.contains("package_visibility_audit_would_block"));
    assert!(log.stdout.contains("root//boundary/strict:t"));
    Ok(())
}

#[buck_test]
async fn test_exemption_does_not_weaken_ancestor_layer(buck: &Buck) {
    buck.ctargets(&["root//outside:uses_marked_legacy"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"`root//marked/inner/legacy:t` is not visible to");
}

#[buck_test]
async fn test_exemption_does_not_lift_inherited_default_visibility(
    buck: &Buck,
) -> anyhow::Result<()> {
    set_default_intersection(buck, "enforce")?;
    buck.ctargets(&["root//outside:uses_legacy_inherits_default"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"`root//boundary/legacy:inherits_default` is not visible to");
    Ok(())
}

#[buck_test]
async fn test_enforce_mode_nested_cell_inherits_parent_layer(buck: &Buck) -> anyhow::Result<()> {
    buck.ctargets(&["root//outside:uses_nested_cell"]).await?;
    set_default_intersection(buck, "enforce")?;
    buck.kill().await?;
    buck.ctargets(&["root//outside:uses_nested_cell"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"`nested//:t` is not visible to");
    Ok(())
}

async fn test_invalid_exemption_is_rejected_impl(buck: &Buck, package: &str, stderr_regex: &str) {
    buck.targets(&[&format!("root//{package}:t")])
        .await
        .expect_failure()
        .assert_stderr_matches(stderr_regex);
}

#[buck_test]
async fn test_invalid_exemption_is_rejected_requires_visibility_requires_an_explicit_visibility(
    buck: &Buck,
) {
    test_invalid_exemption_is_rejected_impl(
        buck,
        "requires_visibility",
        r"requires an explicit `visibility=`",
    )
    .await
}

#[buck_test]
async fn test_invalid_exemption_is_rejected_public_layer_is_vacuous_with_a_visibility_list_containing(
    buck: &Buck,
) {
    test_invalid_exemption_is_rejected_impl(
        buck,
        "public_layer",
        r"is vacuous with a `visibility` list containing",
    )
    .await
}

#[buck_test]
async fn test_invalid_exemption_is_rejected_target_pattern_target_patterns_cell_pkg_name_are_not_accepted(
    buck: &Buck,
) {
    test_invalid_exemption_is_rejected_impl(
        buck,
        "target_pattern",
        r"target patterns \(`cell//pkg:name`\) are not accepted",
    )
    .await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_invalid_exemption_is_rejected_outside_subtree_is_outside_this_PACKAGE_s_subtree(
    buck: &Buck,
) {
    test_invalid_exemption_is_rejected_impl(
        buck,
        "outside_subtree",
        r"is outside this PACKAGE's subtree",
    )
    .await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_invalid_exemption_is_rejected_covers_subtree_covers_this_PACKAGE_s_entire_subtree(
    buck: &Buck,
) {
    test_invalid_exemption_is_rejected_impl(
        buck,
        "covers_subtree",
        r"covers this PACKAGE's entire subtree",
    )
    .await
}

#[buck_test]
async fn test_audit_visibility_lists_would_blocks(buck: &Buck) -> anyhow::Result<()> {
    set_default_intersection(buck, "audit")?;
    let result = buck
        .audit_visibility(&["root//outside:uses_legacy", "root//outside:uses_strict"])
        .await?;
    assert!(
        result
            .stderr
            .contains("would block: root//outside -> root//boundary/strict (1 edges)")
    );
    assert!(
        result
            .stderr
            .contains("1 edges across 1 package pairs would be blocked")
    );
    assert!(!result.stderr.contains("root//boundary/legacy"));
    Ok(())
}

#[buck_test]
async fn test_audit_visibility_audit_mode_still_fails_real_violations(
    buck: &Buck,
) -> anyhow::Result<()> {
    set_default_intersection(buck, "audit")?;
    buck.audit_visibility(&["root//outside:uses_legacy_inherits_default"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"`root//boundary/legacy:inherits_default` is not visible to");
    buck.audit_visibility(&["root//outside:uses_marked_legacy"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"`root//marked/inner/legacy:t` is not visible to");
    Ok(())
}
