/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;

const DUMMY_CONTENT_HASH: &str = "aaaabbbbccccdddd";

// TODO(@wendyy) - remove this config hash hack
// Config hash might change, so let's build a target with linux target platform
// and get the current config hash, which is the 4th index in the buck-out path.
async fn get_config_hash(buck: &Buck, target: &str, args: &[&str]) -> anyhow::Result<String> {
    let mut build_args = vec![target];
    build_args.extend(args);
    let result = buck.build(&build_args).await?;
    let outputs = result
        .build_report()?
        .outputs_for_target_rel(target, "DEFAULT")?;
    let [output] = outputs.as_slice() else {
        anyhow::bail!("`{target}` has {} outputs: {outputs:?}", outputs.len());
    };
    // The build report always separates with `/`; `components` also accepts `\` on Windows.
    let config_hash = output
        .components()
        .nth(4)
        .with_context(|| format!("No config hash in `{}`", output.display()))?
        .as_os_str()
        .to_string_lossy()
        .into_owned();

    Ok(config_hash)
}

/// The one action key of `audit output --output-all-attributes` output.
fn single_action_key(stdout: &str) -> anyhow::Result<String> {
    let action: Value = serde_json::from_str(stdout)?;
    let keys: Vec<&String> = action
        .as_object()
        .context("`audit output` printed no JSON object")?
        .keys()
        .collect();
    assert_eq!(keys.len(), 1);
    Ok(keys[0].clone())
}

#[buck_test]
async fn test_audit_output_malformed_path(buck: &Buck) {
    buck.audit_output(&["blah"])
        .await
        .expect_failure()
        .assert_stderr_matches("Path does not start with buck-out");
}

#[buck_test]
async fn test_audit_output_scratch_path_unsupported(buck: &Buck) -> anyhow::Result<()> {
    // pick a random target, we just want the config hash
    let config_hash = get_config_hash(buck, "root//:dummy", &[]).await?;
    buck.audit_output(&[&format!(
        "buck-out/v2/tmp/cell1/{config_hash}/path/to/target/__target__/output"
    )])
    .await
    .expect_failure()
    .assert_stderr_matches("not supported for audit output");
    Ok(())
}

#[buck_test]
async fn test_audit_output_bxl_unsupported(buck: &Buck) -> anyhow::Result<()> {
    // pick a random target, we just want the config hash
    let config_hash = get_config_hash(buck, "root//:dummy", &[]).await?;
    buck.audit_output(&[&format!(
        "buck-out/v2/art-bxl/cell1/{config_hash}/path/to/function.bxl/__function__/output"
    )])
    .await
    .expect_failure()
    .assert_stderr_matches("not supported for audit output");
    Ok(())
}

#[buck_test]
async fn test_audit_output_anon_targets_unsupported(buck: &Buck) -> anyhow::Result<()> {
    // pick a random target, we just want the config hash
    let config_hash = get_config_hash(buck, "root//:dummy", &[]).await?;
    buck.audit_output(&[&format!(
        "buck-out/v2/art-anon/cell1/{config_hash}/path/to/target/rule_hash/__target__/output"
    )])
    .await
    .expect_failure()
    .assert_stderr_matches("not supported for audit output");
    Ok(())
}

#[buck_test]
async fn test_audit_output_invalid_prefix(buck: &Buck) -> anyhow::Result<()> {
    // invalid prefix (i.e. not art, art-anon, art-bxl, temp, or test)
    // pick a random target, we just want the config hash
    let config_hash = get_config_hash(buck, "root//:dummy", &[]).await?;
    buck.audit_output(&[&format!(
        "buck-out/v2/not_art/cell1/{config_hash}/path/to/target/rule_hash/__target__/output"
    )])
    .await
    .expect_failure()
    .assert_stderr_matches("Malformed buck-out path");
    Ok(())
}

#[buck_test]
async fn test_audit_output_nonexistent_cell(buck: &Buck) -> anyhow::Result<()> {
    // pick a random target, we just want the config hash
    let config_hash = get_config_hash(buck, "root//:dummy", &[]).await?;
    buck.audit_output(&[&format!(
        "buck-out/v2/art/made_up_cell/{config_hash}/path/to/target/rule_hash/__target__/output"
    )])
    .await
    .expect_failure()
    .assert_stderr_matches("unknown cell name");
    Ok(())
}

#[buck_test]
async fn test_audit_output_in_root_directory(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:dummy";
    let config_hash = get_config_hash(buck, target, &[]).await?;
    let result = buck
        .audit_output(&[
            &format!("buck-out/v2/art/root/{config_hash}/__dummy__/foo.txt"),
            "--output-all-attributes",
        ])
        .await?;

    let action_key = single_action_key(&result.stdout)?;
    assert!(action_key.contains(target));
    Ok(())
}

#[buck_test]
async fn test_audit_content_based_output_in_root_directory(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:dummy";
    let result = buck
        .audit_output(&[
            &format!("buck-out/v2/art/root/__dummy__/{DUMMY_CONTENT_HASH}/foo.txt"),
            "-c",
            "test.has_content_based_path=true",
        ])
        .await?;

    assert_eq!(result.stdout.trim(), target);
    Ok(())
}

#[buck_test]
async fn test_non_root_cell(buck: &Buck) -> anyhow::Result<()> {
    let target = "cell1//:dummy2";
    let config_hash = get_config_hash(buck, target, &[]).await?;
    let result = buck
        .audit_output(&[
            &format!("buck-out/v2/art/cell1/{config_hash}/__dummy2__/foo.txt"),
            "--output-all-attributes",
        ])
        .await?;

    let action_key = single_action_key(&result.stdout)?;
    assert!(action_key.contains(target));
    Ok(())
}

#[buck_test]
async fn test_fixed_target_platform(buck: &Buck) -> anyhow::Result<()> {
    let target_platform = "root//:linux_platform";
    let target_platforms_arg = format!("--target-platforms={target_platform}");

    let target = "root//directory:dummy";
    let config_hash = get_config_hash(buck, target, &[&target_platforms_arg]).await?;
    let result = buck
        .audit_output(&[
            &format!("buck-out/v2/art/root/{config_hash}/directory/__dummy__/foo.txt"),
            &target_platforms_arg,
        ])
        .await?;

    let action = &result.stdout;
    assert!(action.contains(target));
    assert!(action.contains(target_platform));
    assert!(action.contains("id"));
    Ok(())
}

#[buck_test]
async fn test_dynamic_output_declared_in_rule_bound_in_dynamic(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//dynamic_output:dynamic_output";
    let config_hash = get_config_hash(buck, target, &[]).await?;

    let result = buck
        .audit_output(&[&format!(
            "buck-out/v2/art/root/{config_hash}/dynamic_output/__dynamic_output__/bound_dynamic.txt"
        )])
        .await?;
    let action = &result.stdout;
    assert!(action.contains(target));
    assert!(action.contains("id"));
    Ok(())
}

#[buck_test]
async fn test_content_based_dynamic_output_declared_in_rule_bound_in_dynamic(
    buck: &Buck,
) -> anyhow::Result<()> {
    let target = "root//dynamic_output:dynamic_output";

    let result = buck
        .audit_output(&[
            &format!(
                "buck-out/v2/art/root/dynamic_output/__dynamic_output__/{DUMMY_CONTENT_HASH}/bound_dynamic.txt"
            ),
            "-c",
            "test.has_content_based_path=true",
        ])
        .await?;
    assert_eq!(result.stdout.trim(), target);
    Ok(())
}

#[buck_test]
async fn test_dynamic_output_declared_and_bound_in_dynamic(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//dynamic_output:dynamic_output";
    let config_hash = get_config_hash(buck, target, &[]).await?;
    let result = buck
        .audit_output(&[&format!(
            "buck-out/v2/art/root/{config_hash}/dynamic_output/__dynamic_output__/defined_dynamic.txt"
        )])
        .await?;
    // FIXME(JakobDegen): Why isn't this an error?
    assert!(
        result
            .stdout
            .contains("Failed to find an action that produced the output path")
    );
    Ok(())
}

#[buck_test]
async fn test_wrong_config_hash(buck: &Buck) -> anyhow::Result<()> {
    // Should return the unconfigured target label
    let target_platform = "root//:linux_platform";
    let target_platforms_arg = format!("--target-platforms={target_platform}");
    let result = buck
        .audit_output(&[
            "buck-out/v2/art/root/aaaabbbbccccdddd/directory/__dummy__/foo.txt",
            &target_platforms_arg,
        ])
        .await?;

    let output = &result.stdout;
    // unconfigure target label should be in the output
    assert!(output.contains("root//directory:dummy"));
    // configuration platform should not be in the output
    assert!(!output.contains(target_platform));
    assert!(output.contains("Platform configuration"));
    assert!(output.contains("did not match"));
    Ok(())
}

#[buck_test]
async fn test_output_directory(buck: &Buck) -> anyhow::Result<()> {
    // Test a rule that outputs to a directory
    let target = "root//directory:empty_dir";
    let config_hash = get_config_hash(buck, target, &[]).await?;
    let result = buck
        .audit_output(&[&format!(
            "buck-out/v2/art/root/{config_hash}/directory/__empty_dir__/outputdir"
        )])
        .await?;

    let action = &result.stdout;
    assert!(action.contains(target));
    assert!(action.contains("id"));
    Ok(())
}

#[buck_test]
async fn test_content_based_output_directory(buck: &Buck) -> anyhow::Result<()> {
    // Test a rule that outputs to a directory
    let target = "root//directory:empty_dir";
    let result = buck
        .audit_output(&[
            &format!("buck-out/v2/art/root/directory/__empty_dir__/{DUMMY_CONTENT_HASH}/outputdir"),
            "-c",
            "test.has_content_based_path=true",
        ])
        .await?;

    assert_eq!(result.stdout.trim(), target);
    Ok(())
}
