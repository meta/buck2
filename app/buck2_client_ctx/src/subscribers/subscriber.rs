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

use async_trait::async_trait;
use buck2_events::BuckEvent;

use crate::console_interaction_stream::ConsoleInteraction;
use crate::exit_result::ExitResult;
use crate::ticker::Tick;

/// Visitor trait.  Implement this to subscribe to the event streams.
/// Each method will be called whenever an event occurs.
#[async_trait]
pub trait EventSubscriber: Send {
    /// Name for debugging, only used for subscribers that have a finalize step.
    fn name(&self) -> &'static str {
        "default"
    }

    /// Fired by the tailer for stdout, or by PartialResultHandler instances that wish to write to
    /// stdout.
    async fn handle_output(&mut self, _raw_output: &[u8]) -> buck2_error::Result<()> {
        Ok(())
    }
    /// Fired by the tailer for stderr.
    async fn handle_tailer_stderr(&mut self, _stderr: &str) -> buck2_error::Result<()> {
        Ok(())
    }
    async fn handle_console_interaction(
        &mut self,
        _c: &ConsoleInteraction,
    ) -> buck2_error::Result<()> {
        Ok(())
    }
    async fn handle_events(&mut self, _event: &[Arc<BuckEvent>]) -> buck2_error::Result<()> {
        Ok(())
    }
    async fn handle_command_result(
        &mut self,
        _result: &buck2_cli_proto::CommandResult,
    ) -> buck2_error::Result<()> {
        Ok(())
    }

    /// Give the subscriber a chance to react to errors as we start trying to clean up.
    /// They may return another error, which will be incorporated into the end result.
    async fn handle_error(&mut self, _error: &buck2_error::Error) -> buck2_error::Result<()> {
        Ok(())
    }

    /// Allow the subscriber to do some sort of action once every render cycle.
    async fn tick(&mut self, _tick: &Tick) -> buck2_error::Result<()> {
        Ok(())
    }

    /// Remove any interactive display (e.g. a superconsole canvas) from the terminal
    /// instead of leaving it behind, for when the display is being replaced rather
    /// than concluded. Subscribers without an interactive display do nothing.
    async fn erase_interactive_output(&mut self) -> buck2_error::Result<()> {
        Ok(())
    }

    /// Return a desired tick rate override. The event loop will adjust its
    /// ticker when this differs from the current rate. Return `None` to keep
    /// the default rate.
    fn desired_ticks_per_second(&self) -> Option<u32> {
        None
    }

    fn handle_stream_end(&mut self) {}
    fn handle_daemon_connection_failure(&mut self) {}
    fn handle_daemon_started(&mut self, _reason: buck2_data::DaemonWasStartedReason) {}

    fn handle_exit_result(&mut self, _result: &ExitResult) {}

    /// Perform final clean up before exiting, upload logs etc.
    async fn finalize(self: Box<Self>) -> buck2_error::Result<()> {
        Ok(())
    }
}
