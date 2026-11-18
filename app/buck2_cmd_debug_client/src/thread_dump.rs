/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::daemon::client::connect::BuckdProcessInfo;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::thread_dump::thread_dump_command;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;

/// Prints a thread dump of the currently running buck daemon to stdout
#[derive(Debug, clap::Parser)]
pub struct ThreadDumpCommand {}

impl ThreadDumpCommand {
    pub fn exec(self, _matches: BuckArgMatches<'_>, ctx: ClientCommandContext<'_>) -> ExitResult {
        ctx.with_runtime(|ctx| async move {
            let daemon_dir = ctx.resolve_tenant_daemon_dir().await?;
            let Ok(info) = BuckdProcessInfo::load(&daemon_dir) else {
                return Err(buck2_error!(ErrorTag::Input, "No running buck daemon"));
            };
            let status = thread_dump_command(&info)?
                .spawn()
                .buck_error_context("Could not run LLDB to grab a thread-dump")?
                .wait()
                .await?;
            if status.success() {
                buck2_error::Ok(ExitResult::success())
            } else {
                // We don't capture stderr, so lldb should have printed an error
                buck2_error::Ok(ExitResult::err(buck2_error!(
                    ErrorTag::Tier0,
                    "Thread dump command failed"
                )))
            }
        })?
    }
}
