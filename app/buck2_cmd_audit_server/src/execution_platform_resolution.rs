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

use async_trait::async_trait;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::execution_platform_resolution::AuditExecutionPlatformResolutionCommand;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::execution_types::execution::ExecutionPlatformIncompatibleReason;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::target_configured_target_label::TargetConfiguredTargetLabel;
use buck2_hash::BuckMutSet;
use buck2_node::attrs::configured_traversal::ConfiguredAttrTraversal;
use buck2_node::attrs::inspect_options::AttrInspectOptions;
use buck2_node::execution::ExecutionPlatformCandidateOutcomes;
use buck2_node::execution::GET_EXECUTION_PLATFORMS;
use buck2_node::execution::GetExecutionPlatforms;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use dupe::Dupe;
use indent_write::io::IndentWriter;

use crate::ServerAuditSubcommand;
use crate::common::configured_target_labels::audit_command_configured_target_labels;

#[async_trait]
impl ServerAuditSubcommand for AuditExecutionPlatformResolutionCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        Ok(server_ctx
            .with_dice_ctx(|server_ctx, ctx| async move {
                let configured_patterns = audit_command_configured_target_labels(
                    &mut ctx.ctx(),
                    &self.patterns,
                    &self.target_cfg,
                    server_ctx,
                )
                .await?;

                let mut stdout = stdout.as_writer();

                match ctx.ctx().get_execution_platforms().await? {
                    None => {
                        writeln!(
                            stdout,
                            "Execution platforms are not configured: the `execution_platforms.target` setting and the `build.execution_platforms` buckconfig are unset"
                        )?;
                        writeln!(stdout, "Using legacy execution platform")?;
                    }
                    Some(platforms) => {
                        writeln!(
                            stdout,
                            "Checking each target against execution platforms defined by {}",
                            platforms.execution_platforms_target()
                        )?;
                    }
                }

                for configured_target in configured_patterns {
                    // This calls `get_internal_configured_target_node` rather than
                    // `get_configured_target_node` because exec platform resolution operates
                    // on `get_internal_configured_target_node`.
                    let configured_node = ctx
                        .ctx()
                        .get_internal_configured_target_node(&configured_target)
                        .await;
                    writeln!(stdout, "{configured_target}:")?;
                    let configured_node = match configured_node {
                        ResultMaybeCompatible::Err(e) => {
                            // Resolution (or configuration) failed. The error deliberately does
                            // not render the skipped platforms, so re-traverse the candidates to
                            // report them.
                            // `:#` renders the full context chain: the outermost context alone
                            // ("Error looking up configured node ...") would hide the root cause.
                            writeln!(stdout, "  Failed to configure: {e:#}")?;
                            let target = TargetConfiguredTargetLabel::new_without_exec_cfg(
                                configured_target.dupe(),
                            );
                            match GET_EXECUTION_PLATFORMS
                                .get()?
                                .diagnose_execution_platform_resolution(&mut ctx.ctx(), &target)
                                .await
                            {
                                Ok(diagnosis) => {
                                    write_candidate_outcomes(&mut stdout, &diagnosis.default)?;
                                    for (group, outcomes) in &diagnosis.execution_groups {
                                        writeln!(stdout, "  Execution group `{group}`:")?;
                                        write_candidate_outcomes(&mut stdout, outcomes)?;
                                    }
                                }
                                Err(e) => writeln!(
                                    stdout,
                                    "  Could not reconstruct skipped platforms: {e}"
                                )?,
                            }
                            continue;
                        }
                        ResultMaybeCompatible::Incompatible(reason) => {
                            writeln!(stdout, "  Incompatible: {reason}")?;
                            continue;
                        }
                        ResultMaybeCompatible::Compatible(node) => node.dupe(),
                    };
                    let resolution = configured_node.execution_platform_resolution();
                    let group_deps = GroupDeps::collect(&configured_node);
                    match resolution.platform() {
                        Ok(platform) => {
                            writeln!(stdout, "  Execution platform: {}", platform.id())?;
                            writeln!(
                                stdout,
                                "    Execution platform configuration: {}",
                                platform.cfg()
                            )?;
                            write_group_deps(&mut stdout, &group_deps, &configured_node, None)?;
                            writeln!(stdout, "    Configuration deps:")?;
                            for config_dep in configured_node.configuration_deps() {
                                writeln!(stdout, "      {}", config_dep.label())?;
                            }
                        }
                        Err(e) => writeln!(stdout, "  {e}")?,
                    }
                    // The error message deliberately does not include the skipped platforms;
                    // this is where they get reported, under the group they were skipped for.
                    for (label, reason) in resolution.skipped() {
                        write_skipped(&mut stdout, label, reason)?;
                    }
                    for (group, platform, skipped) in resolution.execution_group_platforms() {
                        match platform {
                            Some(platform) => writeln!(
                                stdout,
                                "  Execution group `{group}`: {}",
                                platform.id()
                            )?,
                            None => writeln!(
                                stdout,
                                "  Execution group `{group}`: no compatible execution platform"
                            )?,
                        }
                        write_group_deps(&mut stdout, &group_deps, &configured_node, Some(group))?;
                        for (label, reason) in skipped {
                            write_skipped(&mut stdout, label, reason)?;
                        }
                    }
                }

                Ok(())
            })
            .await?)
    }
}

fn write_candidate_outcomes(
    stdout: &mut impl Write,
    outcomes: &buck2_error::Result<ExecutionPlatformCandidateOutcomes>,
) -> buck2_error::Result<()> {
    match outcomes {
        Ok(outcomes) => {
            for (label, outcome) in outcomes {
                match outcome {
                    Ok(()) => writeln!(stdout, "    Compatible {label}")?,
                    Err(reason) => write_skipped(stdout, label, reason)?,
                }
            }
        }
        Err(e) => writeln!(stdout, "    Could not reconstruct skipped platforms: {e:#}")?,
    }
    Ok(())
}

/// The exec and toolchain deps of a configured node, by the execution group (`None` for the
/// default group) of the attribute value that names them.
#[derive(Default)]
struct GroupDeps {
    exec_deps: GroupDepSet,
    toolchain_deps: GroupDepSet,
}

#[derive(Default)]
struct GroupDepSet {
    by_group: BuckMutSet<(Option<NamedExecutionGroup>, ConfiguredTargetLabel)>,
    in_named_group: BuckMutSet<ConfiguredTargetLabel>,
}

impl GroupDepSet {
    fn insert(&mut self, group: Option<&NamedExecutionGroup>, dep: &ConfiguredProvidersLabel) {
        if group.is_some() {
            self.in_named_group.insert(dep.target().dupe());
        }
        self.by_group.insert((group.cloned(), dep.target().dupe()));
    }

    /// Whether `dep` belongs to `group`. A dep that no attribute value names in a named group,
    /// such as an exec dep from a plugin, belongs to the default group.
    fn contains(&self, group: Option<&NamedExecutionGroup>, dep: &ConfiguredTargetLabel) -> bool {
        self.by_group.contains(&(group.cloned(), dep.dupe()))
            || (group.is_none() && !self.in_named_group.contains(dep))
    }
}

impl ConfiguredAttrTraversal for GroupDeps {
    fn dep(&mut self, _dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
        Ok(())
    }

    fn exec_dep(&mut self, dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
        self.exec_deps.insert(None, dep);
        Ok(())
    }

    fn toolchain_dep(&mut self, dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
        self.toolchain_deps.insert(None, dep);
        Ok(())
    }

    fn exec_dep_in_group(
        &mut self,
        dep: &ConfiguredProvidersLabel,
        group: &NamedExecutionGroup,
    ) -> buck2_error::Result<()> {
        self.exec_deps.insert(Some(group), dep);
        Ok(())
    }

    fn toolchain_dep_in_group(
        &mut self,
        dep: &ConfiguredProvidersLabel,
        group: &NamedExecutionGroup,
    ) -> buck2_error::Result<()> {
        self.toolchain_deps.insert(Some(group), dep);
        Ok(())
    }
}

impl GroupDeps {
    fn collect(node: &ConfiguredTargetNode) -> buck2_error::Result<Self> {
        let mut deps = Self::default();
        for attr in node.attrs(AttrInspectOptions::All) {
            attr.traverse(node.label().pkg(), &mut deps)?;
        }
        Ok(deps)
    }
}

/// Writes the node's exec and toolchain deps that belong to `group`, in the node's dep order.
/// Failing to attribute the deps to groups is reported under each group rather than failing the
/// command.
fn write_group_deps(
    stdout: &mut impl Write,
    group_deps: &buck2_error::Result<GroupDeps>,
    node: &ConfiguredTargetNode,
    group: Option<&NamedExecutionGroup>,
) -> buck2_error::Result<()> {
    let group_deps = match group_deps {
        Ok(group_deps) => group_deps,
        Err(e) => {
            writeln!(
                stdout,
                "    Could not list execution deps and toolchain deps: {e:#}"
            )?;
            return Ok(());
        }
    };
    writeln!(stdout, "    Execution deps:")?;
    for dep in node.exec_deps() {
        if group_deps.exec_deps.contains(group, dep.label()) {
            writeln!(stdout, "      {}", dep.label())?;
        }
    }
    writeln!(stdout, "    Toolchain deps:")?;
    for dep in node.toolchain_deps() {
        if group_deps.toolchain_deps.contains(group, dep.label()) {
            writeln!(stdout, "      {}", dep.label())?;
        }
    }
    Ok(())
}

fn write_skipped(
    stdout: &mut impl Write,
    label: &str,
    reason: &ExecutionPlatformIncompatibleReason,
) -> buck2_error::Result<()> {
    writeln!(stdout, "    Skipped {label}")?;
    writeln!(IndentWriter::new("      ", &mut *stdout), "{reason:#}")?;
    Ok(())
}
