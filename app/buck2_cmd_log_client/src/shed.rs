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
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;

mod select_events;

/// Place for commands that may be useful but are too niche but put into user-visible help output
#[derive(Debug, clap::Subcommand)]
pub enum ShedCommand {
    SelectEvents(select_events::SelectEventsCommand),
}

impl ShedCommand {
    pub fn exec(
        self,
        matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        match self {
            Self::SelectEvents(cmd) => ctx.exec(cmd, matches, events_ctx),
        }
    }
}
