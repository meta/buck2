/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Lets code outside this crate watch each streaming command the daemon runs, from start to end.
//!
//! Streaming commands are the ones that send their client events while they run: builds, tests,
//! queries, `targets`, `audit`, `bxl`, `install` and the like. The daemon's direct requests, such as
//! `status`, `kill` and `ping`, are not observed, nor are a few debugging requests with paths of
//! their own, such as `buck2 debug flush-dep-files`, `buck2 debug dice-dump` and
//! `buck2 debug allocative`.
//!
//! The binary sets [`COMMAND_LIFECYCLE_OBSERVER`] at startup, to `None` when it has no observer,
//! and the daemon then does no work for one.

use std::time::SystemTime;

use buck2_cli_proto::ClientContext;
use buck2_cli_proto::CommandResult;
use buck2_common::invocation_paths::TenantPaths;
use buck2_core::fs::project::ProjectRoot;
use buck2_events::BuckEvent;
use buck2_settings::EventForwarding;
use buck2_util::late_binding::LateBinding;
use buck2_wrapper_common::invocation_id::TraceId;

pub static COMMAND_LIFECYCLE_OBSERVER: LateBinding<Option<&'static dyn CommandLifecycleObserver>> =
    LateBinding::new("COMMAND_LIFECYCLE_OBSERVER");

/// Watches the commands the daemon runs.
///
/// Its callbacks, and those of the [`CommandObserver`]s it returns, run on the daemon's own paths
/// for the command. They must return quickly, must not block, and must not panic: do any slow
/// work elsewhere, for example on a spawned task.
pub trait CommandLifecycleObserver: Send + Sync {
    /// Called when the daemon starts running a command, from within the daemon's async runtime.
    /// Returns what watches the rest of the command, or `None` to ignore the command.
    fn command_started(&self, command: &StartedCommand<'_>) -> Option<Box<dyn CommandObserver>>;
}

/// Watches one command after it started.
pub trait CommandObserver: Send {
    /// Called with each of the command's events, in the order the client gets them.
    fn event(&mut self, event: &BuckEvent);

    /// Called with the command's result, the last thing the daemon sends its client. A command
    /// that stops without a result, because its client went away, drops its observer instead.
    fn command_ended(self: Box<Self>, result: &CommandResult);
}

/// What the daemon knows about a command when it starts running it.
pub struct StartedCommand<'a> {
    pub trace_id: &'a TraceId,
    /// The client's command line, sanitized, starting with the program name.
    pub sanitized_argv: &'a [String],
    /// The client's working directory.
    pub working_dir: &'a str,
    pub project_root: &'a ProjectRoot,
    /// The opaque event forwarding keys the client sent with the command.
    pub event_forwarding_keys: &'a [String],
    /// The daemon's `event_forwarding` Buck settings.
    pub event_forwarding_settings: &'a EventForwarding,
    pub start_time: SystemTime,
}

/// The registered observer's interest in one running command. Dropping it without calling
/// [`ObservedCommand::end`] drops the observer without a result.
#[derive(Default)]
pub(crate) struct ObservedCommand(Option<Box<dyn CommandObserver>>);

impl ObservedCommand {
    /// Tells the registered observer, if any, that a command started.
    pub(crate) fn start(
        trace_id: &TraceId,
        client_ctx: &ClientContext,
        paths: &TenantPaths,
        event_forwarding_settings: &EventForwarding,
    ) -> buck2_error::Result<Self> {
        let Some(observer) = *COMMAND_LIFECYCLE_OBSERVER.get()? else {
            return Ok(Self::default());
        };
        Ok(Self(observer.command_started(&StartedCommand {
            trace_id,
            sanitized_argv: &client_ctx.sanitized_argv,
            working_dir: &client_ctx.working_dir,
            project_root: paths.project_root(),
            event_forwarding_keys: &client_ctx.event_forwarding_keys,
            event_forwarding_settings,
            start_time: SystemTime::now(),
        })))
    }

    /// Gives the observer one of the command's events.
    pub(crate) fn event(&mut self, event: &BuckEvent) {
        if let Some(observer) = &mut self.0 {
            observer.event(event);
        }
    }

    /// Gives the observer the command's result, which the daemon sends its client.
    pub(crate) fn end(self, result: &CommandResult) {
        if let Some(observer) = self.0 {
            observer.command_ended(result);
        }
    }
}
