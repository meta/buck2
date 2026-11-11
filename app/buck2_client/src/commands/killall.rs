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
use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::daemon::client::namespaces::prune_exited_unidaemon_namespaces;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_common::daemon_dir::DaemonDir;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_fs::fs_util::uncategorized as fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_wrapper_common::KillallFilter;
use buck2_wrapper_common::is_buck2::WhoIsAsking;
use dupe::Dupe;

/// Kill all buck2 processes on the machine
///
/// By default this kills every buck2 process regardless of repository or isolation dir.
/// Passing `--in-isolation-dir` narrows the kill to processes using that isolation dir, and
/// `--repo` narrows it to processes running in the current repository. Processes that
/// cannot be checked against the requested filter are skipped and reported. The connection
/// information of shared daemons that have exited is removed afterwards.
///
/// Under daemon sharing (`buck2.share_daemon`), `--repo` instead evicts the current repository's
/// tenants from their daemons without stopping a daemon that still serves other tenants. Combine
/// it with `--in-isolation-dir` to evict one tenant.
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
        if project_root.is_some() && ctx.paths()?.routing()?.is_shared() {
            return self.evict_project_tenants(&ctx).await;
        }

        // This client's routing is unshared, but a shared daemon started from this checkout
        // before it opted out may still serve other checkouts.
        let exclude_cwd = match &project_root {
            Some(_) => fs_util::canonicalize_if_exists(ctx.paths()?.daemon_state_dir())?
                .map(|path| path.into_path_buf()),
            None => None,
        };
        let filter = KillallFilter {
            isolation_dir: self.in_isolation_dir,
            project_root,
            exclude_cwd,
        };

        buck2_wrapper_common::killall(WhoIsAsking::Buck2, &filter, |s| {
            let _ignored = buck2_client_ctx::eprintln!("{}", s);
        })
        .then_some(())
        .ok_or(buck2_error::buck2_error!(
            buck2_error::ErrorTag::KillAll,
            "Killall command failed"
        ))?;
        // The connection information of the shared daemons that were just killed names nothing
        // any more; a daemon this could not kill is still judged live and kept. The kill has
        // succeeded by now, so a repository whose configuration cannot be read keeps its stale
        // connection information rather than failing the command.
        match ctx.maybe_paths() {
            Ok(Some(paths)) => prune_exited_unidaemon_namespaces(paths, None).await,
            Ok(None) => {}
            Err(error) => {
                tracing::warn!("Could not prune exited unidaemon namespaces: {error:#}");
            }
        }
        ExitResult::success()
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.event_log_opts
    }
}

impl KillallCommand {
    /// Evicts the repository's tenants from their shared daemons. Each tenant keeps a proxy in
    /// its own daemon directory; killing the proxy evicts the tenant, and a daemon exits with its
    /// final tenant.
    async fn evict_project_tenants(&self, ctx: &ClientCommandContext<'_>) -> ExitResult {
        let legacy = ctx.paths()?.tenant_paths().unshared_daemon_dir()?;
        let root = legacy
            .path
            .parent()
            .internal_error("Legacy slot lacks parent")?;
        if !root.exists() {
            return ExitResult::success();
        }
        let mut evicted = 0;
        let mut failures = 0;
        for entry in std::fs::read_dir(root)? {
            // One slot that cannot be examined must not keep the others from being evicted.
            let inspected = entry.map_err(buck2_error::Error::from).and_then(|entry| {
                let is_dir = entry.file_type()?.is_dir();
                let path = AbsNormPathBuf::try_from(entry.path())?;
                Ok((is_dir, entry.file_name(), path))
            });
            let (is_dir, name, path) = match inspected {
                Ok(inspected) => inspected,
                Err(error) => {
                    failures += 1;
                    buck2_client_ctx::eprintln!(
                        "Cannot inspect a tenant directory under `{}`: {:#}",
                        root,
                        error
                    )?;
                    continue;
                }
            };
            if !is_dir
                || self
                    .in_isolation_dir
                    .as_ref()
                    .is_some_and(|expected| name != std::ffi::OsStr::new(expected))
            {
                continue;
            }
            let dir = DaemonDir { path };
            let shown = dir.to_string();
            if !dir.buckd_info().exists() {
                // An evicted tenant's slot keeps only its lock and logs, and a daemon that died
                // while starting may leave just its pid. A live process named by the pid, unless it
                // started well after the pid was recorded, with no record saying how to reach it
                // is an owner this command cannot evict.
                let recorded_at = std::fs::metadata(dir.buckd_pid().as_path())
                    .and_then(|metadata| metadata.modified())
                    .ok();
                let live_owner = std::fs::read_to_string(dir.buckd_pid().as_path())
                    .ok()
                    .and_then(|pid| pid.trim().parse::<i64>().ok())
                    .filter(|pid| *pid > 0)
                    .and_then(|pid| buck2_wrapper_common::pid::Pid::from_i64(pid).ok())
                    .is_some_and(|pid| {
                        buck2_wrapper_common::kill::recorded_process_may_be_running(
                            pid,
                            0,
                            recorded_at,
                        )
                        .unwrap_or(false)
                    });
                if live_owner {
                    failures += 1;
                    buck2_client_ctx::eprintln!(
                        "Cannot evict the tenant in `{}`: its owner is running but has no `buckd.info`",
                        shown
                    )?;
                }
                continue;
            }
            let result = async {
                let lifecycle =
                    buck2_client_ctx::daemon::client::BuckdLifecycleLock::lock_with_timeout(
                        dir,
                        buck2_client_ctx::startup_deadline::StartupDeadline::duration_from_now(
                            std::time::Duration::from_secs(10),
                        )?,
                    )
                    .await?;
                buck2_client_ctx::daemon::client::kill::kill_unidaemon_tenant(
                    &lifecycle,
                    buck2_client_ctx::daemon::client::kill::TenantShutdownCommand::Repository,
                    None,
                )
                .await
            }
            .await;
            match result {
                Ok(_) => evicted += 1,
                Err(error) => {
                    failures += 1;
                    buck2_client_ctx::eprintln!(
                        "Failed to evict the tenant in `{}`: {:#}",
                        shown,
                        error
                    )?;
                }
            }
        }
        buck2_client_ctx::eprintln!("Evicted {} tenant(s) from the repository", evicted)?;
        if failures != 0 {
            return buck2_error::buck2_error!(
                buck2_error::ErrorTag::KillAll,
                "Repository tenant eviction failed for {failures} tenant(s)"
            )
            .into();
        }
        ExitResult::success()
    }
}
