/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_agent_interactive::Advice;
use buck2_cli_proto::EvictProjectTenantsRequest;
use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::daemon::client::connect::establish_connection_existing;
use buck2_client_ctx::daemon::client::connect::shared_daemon_is_inactive;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_common::daemon_dir::DaemonDir;
use buck2_core::buck2_env;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_fs::fs_util::uncategorized as fs_util;
use buck2_wrapper_common::KillallFilter;
use buck2_wrapper_common::is_buck2::WhoIsAsking;
use dupe::Dupe;
use walkdir::WalkDir;

/// Kill all buck2 processes on the machine
///
/// By default this kills every buck2 process regardless of repository or isolation dir.
/// Passing `--in-isolation-dir` narrows the kill to processes using that isolation dir, and
/// `--repo` narrows it to processes running in the current repository. Processes that
/// cannot be checked against the requested filter are skipped and reported.
///
/// With `BUCK2_SHARE_DAEMON=true`, `--repo` instead evicts the current repository's tenants
/// from their daemons without stopping a daemon that still serves other tenants. Combine it
/// with `--in-isolation-dir` to evict one tenant.
#[derive(Debug, clap::Parser)]
#[clap(verbatim_doc_comment)]
pub struct KillallCommand {
    /// Only kill buck2 processes using this isolation dir.
    ///
    /// Unlike the global `--isolation-dir` flag, this does not default to `v2`; when
    /// omitted, processes of every isolation dir are killed.
    #[clap(long, value_name = "ISOLATION_DIR")]
    in_isolation_dir: Option<String>,

    /// Only kill buck2 processes running in the current repository (project root).
    #[clap(long)]
    repo: bool,

    #[clap(flatten)]
    pub(crate) event_log_opts: CommonEventLogOptions,
}

impl BuckSubcommand for KillallCommand {
    const COMMAND_NAME: &'static str = "killall";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        events_ctx
            .handle_agent_advice(ctx.trace_id.dupe(), ctx.agent_advice(Advice::Killall)?)
            .await?;
        if self.repo && buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)? {
            return self.evict_project_tenants(&ctx, events_ctx).await;
        }

        let project_root = self
            .repo
            .then(|| {
                // Process working directories are read fully resolved from the OS, so
                // canonicalize the root to make the comparison symlink-insensitive.
                let paths = ctx
                    .paths()
                    .buck_error_context("`--repo` requires running from within a repository")?;
                buck2_error::Ok(fs_util::canonicalize(paths.project_root().root())?.into_path_buf())
            })
            .transpose()?;

        let filter = KillallFilter {
            isolation_dir: self.in_isolation_dir,
            project_root,
        };

        buck2_wrapper_common::killall(WhoIsAsking::Buck2, &filter, |s| {
            let _ignored = buck2_client_ctx::eprintln!("{}", s);
        })
        .then_some(())
        .ok_or(buck2_error::buck2_error!(
            buck2_error::ErrorTag::KillAll,
            "Killall command failed"
        ))
        .into()
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.event_log_opts
    }
}

impl KillallCommand {
    async fn evict_project_tenants(
        &self,
        ctx: &ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let context = ctx.empty_client_context(Self::COMMAND_NAME)?;
        let daemon_dir = ctx.paths()?.daemon_dir()?;
        let root = daemon_dir
            .path
            .parent()
            .internal_error("Daemon directory has no parent")?;
        let mut count = 0;
        let mut failures = 0;
        if root.exists() {
            for entry in WalkDir::new(root)
                .min_depth(1)
                .max_depth(1)
                .follow_links(false)
            {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        failures += 1;
                        buck2_client_ctx::eprintln!(
                            "Failed to inspect daemon directory: {:#}",
                            from_any_with_tag(error, buck2_error::ErrorTag::Tier0)
                        )?;
                        continue;
                    }
                };
                if !entry.file_type().is_dir() {
                    continue;
                }
                let dir = DaemonDir {
                    path: entry.into_path().try_into()?,
                };
                if !dir.buckd_info().exists() {
                    if dir.buckd_pid().exists()
                        || dir.buckd_stdout().exists()
                        || dir.buckd_stderr().exists()
                        || dir.buckd_error_log().exists()
                    {
                        failures += 1;
                        buck2_client_ctx::eprintln!(
                            "Cannot inspect daemon `{}`: buckd.info is missing. Restore its metadata before retrying; no process was stopped.",
                            dir
                        )?;
                    }
                    continue;
                }
                let client = match establish_connection_existing(&dir).await {
                    Ok(client) => client,
                    Err(error) => {
                        match shared_daemon_is_inactive(&dir).await {
                            Ok(true) => {}
                            Ok(false) => {
                                failures += 1;
                                buck2_client_ctx::eprintln!(
                                    "Failed to contact daemon `{}`: {:#}",
                                    dir,
                                    error
                                )?;
                            }
                            Err(inactive_error) => {
                                failures += 1;
                                buck2_client_ctx::eprintln!(
                                    "Cannot inspect daemon `{}`: {:#}",
                                    dir,
                                    inactive_error
                                )?;
                            }
                        }
                        continue;
                    }
                };
                let response = client
                    .to_connector()
                    .with_flushing()
                    .evict_project_tenants(
                        EvictProjectTenantsRequest {
                            context: Some(context.clone()),
                            isolation: self.in_isolation_dir.clone(),
                        },
                        events_ctx,
                    )
                    .await;
                let response = match response {
                    Ok(response) => response,
                    Err(error) => {
                        failures += 1;
                        buck2_client_ctx::eprintln!(
                            "Failed to evict tenants from daemon `{}`: {:#}",
                            dir,
                            error
                        )?;
                        continue;
                    }
                };
                count += response.generations.len();
                if response.shutting_down {
                    buck2_client_ctx::eprintln!("Final tenant evicted; shutting down daemon")?;
                }
            }
        }
        buck2_client_ctx::eprintln!("Evicted {} tenant(s) from the repository", count)?;
        if failures != 0 {
            return buck2_error::buck2_error!(
                buck2_error::ErrorTag::KillAll,
                "Repository tenant eviction failed for {failures} daemon(s)"
            )
            .into();
        }
        ExitResult::success()
    }
}
