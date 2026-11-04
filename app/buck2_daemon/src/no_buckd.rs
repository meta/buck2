/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_client_ctx::daemon::client::BuckdLifecycleLock;
use buck2_client_ctx::daemon::client::connect::buckd_startup_init_timeout;
use buck2_client_ctx::daemon::client::connect::buckd_startup_timeout;
use buck2_client_ctx::daemon::client::kill::kill_command_impl;
use buck2_client_ctx::startup_deadline::StartupDeadline;
use buck2_common::init::DaemonStartupConfig;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_core::logging::LogConfigurationReloadHandle;
use buck2_error::buck2_error;
use buck2_util::threads::thread_spawn;

use crate::daemon::DaemonCommand;

pub fn start_in_process_daemon(
    daemon_startup_config: &DaemonStartupConfig,
    paths: InvocationPaths,
    runtime: &tokio::runtime::Runtime,
) -> buck2_error::Result<Option<Box<dyn FnOnce() -> buck2_error::Result<()> + Send + Sync>>> {
    let handle = runtime.handle().clone();
    let daemon_startup_config = daemon_startup_config.clone();
    // Create a function which kills an existing daemon, and then spawns an in-process one.
    Ok(Some(Box::new(move || {
        // The in-process daemon replaces any resident one, so the first command that actually
        // connects kills it, right before starting its own. For a command that never connects
        // to a daemon `--no-buckd` is a no-op.
        //
        // With no daemon recorded there is nothing to replace, and `kill` would only report
        // that on every invocation.
        let daemon_dir = paths.daemon_dir()?;
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let lifecycle_lock = BuckdLifecycleLock::lock_with_timeout(
                    daemon_dir,
                    StartupDeadline::duration_from_now(buckd_startup_timeout()?)?,
                )
                .await?;

                kill_command_impl(
                    &lifecycle_lock,
                    "A command with `--no-buckd` is invoked",
                    true,
                    std::time::Duration::ZERO,
                )
                .await
            })
        })?;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // Spawn a thread which runs the daemon.
        thread_spawn("buck2-no-buckd", move || {
            let tx_clone = tx.clone();
            let result = DaemonCommand::new_in_process(daemon_startup_config).exec(
                <dyn LogConfigurationReloadHandle>::noop(),
                paths,
                true,
                Box::new(move || drop(tx_clone.send(Ok(())))),
            );
            // Since `tx` is unbounded, there's race here: it is possible
            // that error message will be lost in the channel and not reported anywhere.
            // Not an issue practically, because daemon does not usually error
            // after it started listening.
            if let Err(e) = tx.send(result) {
                match e.0 {
                    Ok(()) => drop(buck2_client_ctx::eprintln!(
                        "In-process daemon gracefully stopped"
                    )),
                    Err(e) => drop(buck2_client_ctx::eprintln!(
                        "In-process daemon run failed: {:#}",
                        e
                    )),
                }
            }
        })?;
        // Wait for the daemon to serve (or to fail). Like a spawned daemon, it gets the
        // initialization timeout for that: the client connects with the much shorter timeout
        // for an existing daemon.
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                StartupDeadline::duration_from_now(buckd_startup_init_timeout()?)?
                    .run("starting in-process daemon", async {
                        match rx.recv().await {
                            Some(r) => r,
                            None => Err(buck2_error!(
                                buck2_error::ErrorTag::Tier0,
                                "In-process daemon failed to start and we don't know why"
                            )),
                        }
                    })
                    .await
            })
        })
    })))
}
