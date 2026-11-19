/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_cli_proto::ClientContext;
use buck2_cli_proto::new_generic::CompleteRequest;
use buck2_cli_proto::new_generic::NewGenericRequest;
use buck2_cli_proto::new_generic::NewGenericResponse;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::command_outcome::CommandOutcome;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::FlushingBuckdClient;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::streaming::StreamingCommand;
use buck2_common::invocation_roots::InvocationRoots;
use buck2_common::legacy_configs::cells::BuckConfigBasedCells;
use buck2_fs::working_dir::AbsWorkingDir;
use futures::FutureExt;
use futures::future::LocalBoxFuture;

use super::path_sanitizer::PathSanitizer;
use super::results::CompletionResults;
use crate::complete::print_completions;

pub(crate) trait TargetResolver: Send {
    fn resolve(
        &mut self,
        partial_target: String,
    ) -> LocalBoxFuture<'_, CommandOutcome<Vec<String>>>;
}

pub(crate) struct CompleteTargetCommand {
    target_cfg: TargetCfgOptions,

    cwd: AbsWorkingDir,
    package: String,
    partial_target: String,
}

impl CompleteTargetCommand {
    pub(crate) fn new(cwd: &AbsWorkingDir, package: String, partial_target: String) -> Self {
        let target_cfg = TargetCfgOptions::default();
        Self {
            target_cfg,
            cwd: cwd.to_owned(),
            package,
            partial_target,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl StreamingCommand for CompleteTargetCommand {
    const COMMAND_NAME: &'static str = "complete";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let buckd_client = buckd.with_flushing();
        let mut context = ctx.client_context(matches, &self)?;
        context.reuse_current_config_implicitly = true;
        let mut target_resolver = DaemonTargetResolver {
            buckd_client,
            context,
            target_cfg: self.target_cfg,
            events_ctx,
        };

        let completer = TargetCompleter::new(&self.cwd, &ctx.paths()?.roots, &mut target_resolver)
            .await
            .expect("Failed to create target completer");
        let task = completer.complete(&self.package, &self.partial_target);

        match task.await {
            CommandOutcome::Success(completions) => {
                print_completions(CommandOutcome::Success(completions))
            }
            CommandOutcome::Failure(err) => err,
        }
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        CommonConsoleOptions::none_ref()
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        CommonEventLogOptions::default_ref()
    }

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions {
        CommonBuildConfigurationOptions::reuse_current_config_and_preemptible_ref()
    }

    fn starlark_opts(&self) -> &CommonStarlarkOptions {
        CommonStarlarkOptions::default_ref()
    }
}
pub(crate) struct TargetCompleter<'a> {
    cwd: AbsWorkingDir,
    roots: &'a InvocationRoots,
    cell_configs: Arc<BuckConfigBasedCells>,
    target_resolver: &'a mut dyn TargetResolver,
    results: CompletionResults<'a>,
}

impl<'a> TargetCompleter<'a> {
    pub(crate) async fn new(
        cwd: &AbsWorkingDir,
        roots: &'a InvocationRoots,
        target_resolver: &'a mut dyn TargetResolver,
    ) -> buck2_error::Result<Self> {
        let cell_configs =
            Arc::new(BuckConfigBasedCells::parse_with_config_args(&roots.project_root, &[]).await?);
        Ok(Self {
            cwd: cwd.to_owned(),
            roots,
            cell_configs: cell_configs.clone(),
            target_resolver,
            results: CompletionResults::new(roots, cell_configs.clone()),
        })
    }

    /// Complete the target in a partial label.
    ///
    /// Returns a collection of possible completions, each including the partial
    /// target including the cell/package name(s).
    pub(crate) async fn complete(
        mut self,
        given_package: &str,
        partial_target: &str,
    ) -> CommandOutcome<Vec<String>> {
        let sanitizer = PathSanitizer::new(&self.cell_configs, &self.cwd, self.roots).await?;
        let path = sanitizer.sanitize(given_package)?;
        let completions = self
            .target_resolver
            .resolve(path.given().to_owned() + ":")
            .await?;

        for label in completions {
            let target = label.split(':').next_back().unwrap();
            if target.starts_with(partial_target) {
                let completion = path.given().to_owned() + ":" + target;
                self.results.insert(&completion);
            }
        }
        CommandOutcome::Success(self.results.into())
    }
}

struct DaemonTargetResolver<'a> {
    buckd_client: FlushingBuckdClient<'a>,
    context: ClientContext,
    target_cfg: TargetCfgOptions,
    events_ctx: &'a mut EventsCtx,
}

impl TargetResolver for DaemonTargetResolver<'_> {
    fn resolve(
        &mut self,
        partial_target: String,
    ) -> LocalBoxFuture<'_, CommandOutcome<Vec<String>>> {
        let request = NewGenericRequest::Complete(CompleteRequest {
            target_cfg: self.target_cfg.target_cfg(),
            partial_target,
        });
        self.buckd_client
            .new_generic(self.context.clone(), request, self.events_ctx, None)
            .then(|res| async move {
                match res {
                    Ok(CommandOutcome::Success(NewGenericResponse::Complete(res))) => {
                        CommandOutcome::Success(res.completions)
                    }
                    Ok(CommandOutcome::Success(_)) => CommandOutcome::Failure(ExitResult::bail(
                        "Unexpected response type from generic command",
                    )),
                    Ok(CommandOutcome::Failure(result)) => CommandOutcome::Failure(result),
                    Err(e) => CommandOutcome::Failure(ExitResult::err(e)),
                }
            })
            .boxed_local()
    }
}
