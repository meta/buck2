/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::io::Write;

use async_trait::async_trait;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::visibility::AuditVisibilityCommand;
use buck2_common::pattern::parse_from_cli::parse_patterns_from_cli_args;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_node::load_patterns::MissingTargetBehavior;
use buck2_node::load_patterns::load_patterns;
use buck2_node::nodes::lookup::TargetNodeLookup;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_node::package_visibility::HasPackageVisibilityDefaultIntersection;
use buck2_query::query::environment::QueryTargetDepsSuccessors;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_query::query::traversal::async_depth_first_postorder_traversal;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_settings::PackageVisibilityDefaultIntersection;
use dice::DiceTransaction;
use dupe::Dupe;
use futures::FutureExt;

use crate::ServerAuditSubcommand;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
enum VisibilityCommandError {
    #[error(
        "Internal Error: The dependency `{0}` of the target `{1}` was not found during the traversal."
    )]
    DepNodeNotFound(String, String),
}

/// Under `audit`, edges blocked only by `Default` intersection layers are
/// listed per `(consumer package, dep package)` instead of failing. Unlike the
/// build-time soft errors, this sees every edge regardless of DICE caching.
async fn verify_visibility(
    server_ctx: &dyn ServerCommandContextTrait,
    ctx: DiceTransaction,
    targets: TargetSet<TargetNode>,
) -> buck2_error::Result<()> {
    let audit = ctx
        .per_transaction_data()
        .get_package_visibility_default_intersection()
        == PackageVisibilityDefaultIntersection::Audit;
    let mut new_targets: TargetSet<TargetNode> = TargetSet::new();

    let visit = |target| {
        new_targets.insert(target);
        Ok(())
    };

    ctx.ctx()
        .with_linear_recompute(|ctx| {
            async move {
                let lookup = TargetNodeLookup(ctx);

                async_depth_first_postorder_traversal(
                    &lookup,
                    targets.iter_names(),
                    QueryTargetDepsSuccessors,
                    visit,
                    false, // allow_partial_graph
                )
                .await
            }
            .boxed()
        })
        .await?;

    let mut visibility_errors = Vec::new();
    let mut would_block: BTreeMap<(PackageLabel, PackageLabel), usize> = BTreeMap::new();

    for target in new_targets.iter() {
        for dep in target.deps() {
            match new_targets.get(dep) {
                Some(val) => {
                    if val.is_visible_to(target.label())? {
                        continue;
                    }
                    if audit && val.is_visible_to_ignoring_default_layers(target.label())? {
                        *would_block
                            .entry((target.label().pkg(), val.label().pkg()))
                            .or_default() += 1;
                    } else {
                        visibility_errors.push(val.not_visible_to_error(target.label().dupe()));
                    }
                }
                None => {
                    return Err(buck2_error::Error::from(
                        VisibilityCommandError::DepNodeNotFound(
                            dep.to_string(),
                            target.label().name().to_string(),
                        ),
                    ));
                }
            }
        }
    }

    // The client prints what it is sent line by line, as it arrives. A line goes out in one
    // write, so that the writer cannot send it in two parts when its buffer fills.
    let mut stderr = server_ctx.stderr()?;
    let mut line = |line: String| stderr.write_all(format!("{line}\n").as_bytes());

    if !would_block.is_empty() {
        for ((consumer, dep), edges) in &would_block {
            line(format!("would block: {consumer} -> {dep} ({edges} edges)"))?;
        }
        line(format!(
            "{} edges across {} package pairs would be blocked under `package_visibility.default_intersection = \"enforce\"`",
            would_block.values().sum::<usize>(),
            would_block.len(),
        ))?;
    }

    for err in &visibility_errors {
        line(err.to_string())?;
    }

    if !visibility_errors.is_empty() {
        return Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "{}",
            1
        ));
    }

    line("audit visibility succeeded".to_owned())?;
    Ok(())
}

#[async_trait]
impl ServerAuditSubcommand for AuditVisibilityCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        _stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        Ok(server_ctx
            .with_dice_ctx(|server_ctx, ctx| async move {
                let parsed_patterns = parse_patterns_from_cli_args::<TargetPatternExtra>(
                    &mut ctx.ctx(),
                    &self.patterns,
                    server_ctx.working_dir(),
                )
                .await?;

                let parsed_target_patterns =
                    load_patterns(&mut ctx.ctx(), parsed_patterns, MissingTargetBehavior::Fail)
                        .await?;

                let mut nodes = TargetSet::<TargetNode>::new();
                for (_package, result) in parsed_target_patterns.iter() {
                    let res = result.as_ref().map_err(Dupe::dupe)?;
                    nodes.extend(res.values().map(|n| n.to_owned()));
                }

                verify_visibility(server_ctx, ctx, nodes).await?;
                Ok(())
            })
            .await?)
    }
}
