/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;

#[buck_test]
async fn test_docs_agent_does_not_start_daemon(buck: &Buck) -> anyhow::Result<()> {
    buck.docs(&["agent"]).await?;

    let status = buck.status(&[]).await?;
    assert!(status.stderr.contains("no buckd running"));
    Ok(())
}

#[buck_test]
async fn test_docs_agent_preserves_daemon_config(buck: &Buck) -> anyhow::Result<()> {
    buck.audit_config(&["--config", "foo.bar=bar"]).await?;
    let version_path = [
        "Event",
        "data",
        "SpanStart",
        "data",
        "CommandCritical",
        "dice_version",
    ];
    let before = filter_events(buck, &version_path).await?;
    assert_eq!(before.len(), 1);

    buck.docs(&["agent"]).await?;

    let result = buck
        .audit_config(&["foo.bar", "--reuse-current-config", "--style", "json"])
        .await?;
    assert_eq!(
        serde_json::Value::Object(result.audit_config_json()?),
        serde_json::json!({"foo.bar": "bar"})
    );
    assert_eq!(filter_events(buck, &version_path).await?, before);

    buck.audit_config(&["--config", "foo.bar=bar"]).await?;
    assert_eq!(filter_events(buck, &version_path).await?, before);
    assert_eq!(
        filter_events(
            buck,
            &["Event", "data", "Instant", "data", "CellHasNewConfigs"]
        )
        .await?,
        Vec::<serde_json::Value>::new()
    );
    Ok(())
}

#[buck_test]
async fn test_docs_agent_reads_includes_and_overrides(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join("guidance.bcfg"),
        "[agent_context#included]\n description = From the included file\n",
    )?;
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(b"\n<file:guidance.bcfg>\n")?;
    std::fs::create_dir(buck.cwd().join("nested"))?;

    let result = buck.docs(&["agent"]).rel_cwd("nested").await?;
    assert!(
        result
            .stdout
            .contains("Description: From the included file")
    );

    let overrides = buck.cwd().join("overrides.bcfg");
    std::fs::write(
        &overrides,
        "[agent_context#included]\n description = From the config override file\n",
    )?;
    let overrides = overrides.to_string_lossy();
    let flag = "agent_context#included.description=From the command line";
    let result = buck
        .docs(&["agent", "--config-file", &overrides, "--config", flag])
        .await?;
    assert!(result.stdout.contains("Description: From the command line"));
    assert!(!result.stdout.contains("From the config override file"));

    let result = buck
        .docs(&["agent", "--config", flag, "--config-file", &overrides])
        .await?;
    assert!(
        result
            .stdout
            .contains("Description: From the config override file")
    );
    assert!(!result.stdout.contains("From the command line"));
    Ok(())
}

#[buck_test]
async fn test_docs_agent_prints_agent_context_schema(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.docs(&["agent"]).await?;

    assert_eq!(
        result.stdout,
        r#"For builds at Meta, use Buck2.

# Rules

ALWAYS pass `--agent-context direct_call=true` for every `buck2` or `buck` command you invoke directly.

Also provide `--agent-context key=value` entries following the repository-defined schema below. For multiple entries, repeat the flag: `--agent-context direct_call=true --agent-context key=value`.

# `--agent-context` schema

Fields:
- `attempt`
  Required: true
  Description: Which attempt number this is for the same logical task
  Allowed values: any
- `intent`
  Required: true
  Description: The purpose of this Buck2 invocation
  Allowed values: build, test, query, fix, investigate
- `prior_error`
  Required: false
  Description: The error category from the previous failed build
  Allowed values: kotlin_unresolved_reference, missing_target, starlark_error, action_command_failure
"#
    );
    Ok(())
}
