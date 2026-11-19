/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::BuckCommand;
use buck2_e2e::prelude::*;

// TODO(iguridi) or TODO(raulgarcia4):
// New `audit` commands have been added since these tests were created.
// Test them if necessary.

type AuditCommand = for<'a> fn(&'a Buck, &[&str]) -> BuckCommand<'a>;

async fn test_pass_common_opts_func(buck: &Buck, cmd: AuditCommand) -> anyhow::Result<()> {
    cmd(buck, &["--client-metadata", "id=placeholder_id"]).await?;
    Ok(())
}

#[buck_test]
async fn test_pass_common_opts_func_audit_visibility(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts_func(buck, Buck::audit_visibility).await
}

#[buck_test]
async fn test_pass_common_opts_func_audit_configurations(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts_func(buck, Buck::audit_configurations).await
}

#[buck_test]
async fn test_pass_common_opts_func_audit_config(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts_func(buck, Buck::audit_config).await
}

#[buck_test]
async fn test_pass_common_opts_func_audit_visibility_2(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts_func(buck, Buck::audit_visibility).await
}

async fn test_pass_common_opts(buck: &Buck, cmd: &str) -> anyhow::Result<()> {
    let commands_requiring_target_pattern_arg_value = ["providers", "subtargets"];

    if commands_requiring_target_pattern_arg_value.contains(&cmd) {
        buck.audit(&[cmd, "//:dummy", "--client-metadata", "id=placeholder_id"])
            .await?;
    } else {
        buck.audit(&[cmd, "--client-metadata", "id=placeholder_id"])
            .await?;
    }
    Ok(())
}

#[buck_test]
async fn test_pass_common_opts_analysis_queries(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts(buck, "analysis-queries").await
}

#[buck_test]
async fn test_pass_common_opts_cell(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts(buck, "cell").await
}

#[buck_test]
async fn test_pass_common_opts_execution_platform_resolution(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts(buck, "execution-platform-resolution").await
}

#[buck_test]
async fn test_pass_common_opts_includes(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts(buck, "includes").await
}

#[buck_test]
async fn test_pass_common_opts_prelude(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts(buck, "prelude").await
}

#[buck_test]
async fn test_pass_common_opts_providers(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts(buck, "providers").await
}

#[buck_test]
async fn test_pass_common_opts_subtargets(buck: &Buck) -> anyhow::Result<()> {
    test_pass_common_opts(buck, "subtargets").await
}
