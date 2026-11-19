/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use buck2_e2e::prelude::*;

#[buck_test(allow_soft_errors = true)]
async fn test_reuse_current_config_with_config_overrides_and_previous_invocation(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let result_file = buck.audit_config(&["test.key", "--style", "json"]).await?;

    assert_eq!(
        result_file.audit_config_json()?.get("test.key"),
        Some(&serde_json::json!("val"))
    );

    let config_override = tmp_path.join("config_override.bcfg");
    std::fs::write(&config_override, "[test]\n  key = override\n")?;

    let result_file = buck
        .audit_config(&[
            "--config-file",
            &config_override.to_string_lossy(),
            "--config",
            "test.key2=override2",
            "--reuse-current-config",
            "--style",
            "json",
        ])
        .await?;

    let json = result_file.audit_config_json()?;
    assert_eq!(json.get("test.key"), Some(&serde_json::json!("val")));
    assert!(json.get("test.key2").is_none_or(serde_json::Value::is_null));
    assert!(result_file.stderr.contains("using current config instead"));
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
async fn test_reuse_current_config_with_config_overrides_and_no_previous_invocation(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result_file = buck
        .audit_config(&[
            "--config",
            "test.key=override",
            "--style",
            "json",
            "--reuse-current-config",
        ])
        .await?;
    assert_eq!(
        result_file.audit_config_json()?.get("test.key"),
        Some(&serde_json::json!("override"))
    );
    assert!(
        result_file
            .stderr
            .contains("Ignoring --reuse-current-config flag")
    );
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
async fn test_reuse_current_config_with_no_previous_invocation(buck: &Buck) -> anyhow::Result<()> {
    let result_file = buck
        .audit_config(&["test.key", "--style", "json", "--reuse-current-config"])
        .await?;
    assert_eq!(
        result_file.audit_config_json()?.get("test.key"),
        Some(&serde_json::json!("val"))
    );
    assert!(
        result_file
            .stderr
            .contains("Ignoring --reuse-current-config flag")
    );
    Ok(())
}
