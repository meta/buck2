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
use buck2_client_ctx::exit_result::ExitResult;

/// Print buck2 daemon directory (`~/.buckd/xxx`).
#[derive(Debug, clap::Parser)]
pub struct DaemonDirCommand {}

impl DaemonDirCommand {
    pub fn exec(self, _matches: BuckArgMatches<'_>, ctx: ClientCommandContext<'_>) -> ExitResult {
        let daemon_dir =
            ctx.with_runtime(|ctx| async move { ctx.resolve_tenant_daemon_dir().await })?;
        buck2_client_ctx::println!("{}", daemon_dir.path.display())?;
        ExitResult::success()
    }
}
