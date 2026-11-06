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

#[buck_test(inplace = false, allow_soft_errors = true)]
async fn test_unified_constraint_defination(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//test_unified_constraint.bxl:main"]).await?;
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_unified_constraint_miss_default_fail(buck: &Buck) {
    buck.targets(&["//miss_default:"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            ".*Missing named-only parameter `default` for call to `constraint`.*",
        );
}

#[buck_test(inplace = false)]
async fn test_unified_constraint_default_not_appear_in_value_fail(buck: &Buck) {
    buck.audit(&["subtargets", "//default_value_not_appear:"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r#".*default value 'linux' must be one of the declared values: \["macos", "windows"\].*"#,
        );
}

// 'default' as a value is allowed only when it is also the default value (see the `compiler`
// constraint in the root fixture). Here it is a value but NOT the default, so it must fail as ambiguous.
// Distinct test names are used because test discovery treats 'default' and 'DEFAULT' as the same name
// (case-insensitive).
#[buck_test(inplace = false)]
async fn test_unified_constraint_reserved_keyword_default_lowercase_fail(buck: &Buck) {
    buck.audit(&["subtargets", "//reserved_keyword_default_lowercase:"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            ".*'default' can be used as a constraint value only when it is also the default value.*",
        );
}

// Test reserved keyword 'DEFAULT' (uppercase)
#[buck_test(inplace = false)]
async fn test_unified_constraint_reserved_keyword_default_uppercase_fail(buck: &Buck) {
    buck.audit(&["subtargets", "//reserved_keyword_default_uppercase:"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            ".*'DEFAULT' is a reserved keyword and cannot be used as a constraint value.*",
        );
}

#[buck_test(inplace = false, allow_soft_errors = true)]
async fn test_unified_constraint_cfg_transition(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//test_unified_constraint.bxl:test_cfg_transition"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, allow_soft_errors = true)]
async fn test_unified_constraint_cfg_transition_v2(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//test_unified_constraint.bxl:test_cfg_transition_v2"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, allow_soft_errors = true)]
async fn test_unified_constraint_for_constraint_v2(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//test_unified_constraint.bxl:constraint_v2"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_unified_constraint_single_value_without_flag_fail(buck: &Buck) {
    buck.audit(&["subtargets", "//single_value_no_flag:"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"constraint\(\) rule must have at least two values");
}

#[buck_test(inplace = false)]
async fn test_unified_constraint_extend_with_constraint_value(buck: &Buck) -> anyhow::Result<()> {
    for (platform, expected) in [("ps5", "ps5"), ("macos", "macos")] {
        let result = buck
            .cquery(&[
                "//extend_with_constraint_value:foo",
                "--target-platforms",
                &format!("//extend_with_constraint_value:{platform}_platform"),
                "--output-attribute",
                "os",
            ])
            .await?;
        assert!(
            result.stdout.contains(&format!(r#""os": "{expected}""#)),
            "{}",
            result.stdout
        );
    }
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_unified_constraint_single_value_with_flag(buck: &Buck) -> anyhow::Result<()> {
    buck.audit(&["subtargets", "//single_value_with_flag:"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_unified_constraint_zero_values_with_flag_fail(buck: &Buck) {
    buck.audit(&["subtargets", "//zero_values_with_flag:"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"constraint\(\) rule must have at least one value");
}

#[buck_test(inplace = false)]
async fn test_unified_constraint_alias_conflict_with_value_fail(buck: &Buck) {
    buck.audit(&["subtargets", "//alias_conflict_with_value:"])
        .await
        .expect_failure()
        .assert_stderr_matches("alias 'v1' conflicts with a declared constraint value");
}

#[buck_test(inplace = false)]
async fn test_unified_constraint_alias_value_not_declared_fail(buck: &Buck) {
    buck.audit(&["subtargets", "//alias_value_not_declared:"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r#"alias 'stable' targets value 'v3' which is not declared in values: \[\"v1\", \"v2\"\]"#,
        );
}

#[buck_test(inplace = false)]
async fn test_unified_constraint_alias_reserved_keyword_fail(buck: &Buck) {
    buck.audit(&["subtargets", "//alias_reserved_keyword:"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "alias 'default' is a reserved keyword and cannot be used as a constraint alias",
        );
}
