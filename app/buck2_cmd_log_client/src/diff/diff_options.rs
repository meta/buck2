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
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::path_arg::PathArg;
use buck2_event_log::read::EventLogPathBuf;
use buck2_wrapper_common::invocation_id::TraceId;

#[derive(Debug, clap::Parser)]
#[clap(group = clap::ArgGroup::new("first").required(true))]
#[clap(group = clap::ArgGroup::new("second").required(true))]
pub struct DiffEventLogOptions {
    /// A path to an event-log file of the first command.
    #[clap(long = "path1", group = "first")]
    path1: Option<PathArg>,
    /// Trace id of the first command.
    #[clap(long = "trace-id1", group = "first")]
    trace_id1: Option<TraceId>,
    /// Open the event-log file from a recent command for the first command.
    #[clap(long, group = "first", value_name = "NUMBER")]
    recent1: Option<usize>,
    /// A path to an event-log file of the second command.
    #[clap(long = "path2", group = "second")]
    path2: Option<PathArg>,
    /// Trace id of the second command.
    #[clap(long = "trace-id2", group = "second")]
    trace_id2: Option<TraceId>,
    /// Open the event-log file from a recent command for the second command.
    #[clap(long, group = "second", value_name = "NUMBER")]
    recent2: Option<usize>,
}

impl DiffEventLogOptions {
    pub(crate) async fn get(
        &self,
        ctx: &ClientCommandContext<'_>,
    ) -> buck2_error::Result<(EventLogPathBuf, EventLogPathBuf)> {
        let options1 = &EventLogOptions {
            recent: self.recent1,
            path: self.path1.clone(),
            trace_id: self.trace_id1.clone(),
            no_remote: false,
            allow_remote: true,
        };
        let options2 = &EventLogOptions {
            recent: self.recent2,
            path: self.path2.clone(),
            trace_id: self.trace_id2.clone(),
            no_remote: false,
            allow_remote: true,
        };

        let log_path1 = options1.get(ctx).await?;
        let log_path2 = options2.get(ctx).await?;

        Ok((log_path1, log_path2))
    }
}
