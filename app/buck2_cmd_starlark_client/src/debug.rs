/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;
use std::sync::LazyLock;

use async_trait::async_trait;
use buck2_cli_proto::DapRequest;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::common::ui::ConsoleType;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::events_ctx::PartialResultCtx;
use buck2_client_ctx::events_ctx::PartialResultHandler;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::ide_support::ide_message_stream;
use buck2_client_ctx::stream_util::reborrow_stream_for_static;
use buck2_client_ctx::streaming::StreamingCommand;
use buck2_client_ctx::subscribers::subscriber::EventSubscriber;
use buck2_event_observer::unpack_event::UnpackedBuckEvent;
use buck2_event_observer::unpack_event::unpack_event;
use buck2_events::BuckEvent;
use futures::StreamExt;

/// Run the starlark debug adapter protocol server
///
/// This forwards requests received on stdin to a debug server running in the
/// buck daemon. DAP events and responses are returned from the daemon and sent
/// to this command's stdout.
#[derive(Debug, clap::Parser)]
#[clap(name = "starlark-debug-attach")]
pub struct StarlarkDebugAttachCommand {
    #[clap(flatten)]
    config_opts: CommonBuildConfigurationOptions,

    #[clap(flatten)]
    starlark_opts: CommonStarlarkOptions,

    #[clap(flatten)]
    event_log_opts: CommonEventLogOptions,
}

pub fn write_dap_message(out: &mut impl Write, msg: &[u8]) -> buck2_error::Result<()> {
    write!(out, "Content-Length: {}\r\n\r\n", msg.len())?;
    out.write_all(msg)?;
    out.flush()?;
    Ok(())
}

/// All DAP messages are written to stdout.
fn send_message_to_dap_client(msg: &[u8]) -> buck2_error::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    write_dap_message(&mut stdout, msg)?;
    Ok(())
}

#[async_trait(?Send)]
impl StreamingCommand for StarlarkDebugAttachCommand {
    const COMMAND_NAME: &'static str = "starlark-debug-attach";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let client_context = ctx.client_context(matches, &self)?;

        let stream = ide_message_stream::<_, debugserver_types::Request>(ctx.stdin()).filter_map(
            |m| async move {
                match m {
                    Ok(dap_json) => Some(DapRequest { dap_json }),
                    Err(e) => {
                        let _ignored = buck2_client_ctx::eprintln!(
                            "Could not read message from stdin: `{}`",
                            e
                        );
                        // TODO(cjhopman): the client just hangs at this point. We should probably error out (or
                        // distinguish between FramedRead errors and errors of us converting to a Request).
                        None
                    }
                }
            },
        );

        let mut partial_result_handler = DapPartialResultHandler;

        reborrow_stream_for_static(
            stream,
            |stream| async move {
                buckd
                    .with_flushing()
                    .dap(
                        client_context,
                        stream,
                        events_ctx,
                        &mut partial_result_handler,
                    )
                    .await
            },
            // The DAP server side does not handle hangups. So, until it does... we never hang up:
            || None,
            || None,
        )
        .await??;

        ExitResult::success()
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        // This should only be communicated with by an IDE, so disable anything other
        // than the simple console
        static SIMPLE_CONSOLE: LazyLock<CommonConsoleOptions> =
            LazyLock::new(|| CommonConsoleOptions {
                console_type: ConsoleType::Simple,
                ui: vec![],
                no_interactive_console: true,
            });
        &SIMPLE_CONSOLE
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.event_log_opts
    }

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions {
        &self.config_opts
    }

    fn starlark_opts(&self) -> &CommonStarlarkOptions {
        &self.starlark_opts
    }

    fn should_expect_spans(&self) -> bool {
        // If we're running the debugger, do not show "Waiting for daemon..." if we do not get any spans.
        false
    }

    fn extra_subscribers(&self) -> Vec<Box<dyn EventSubscriber>> {
        /// We add an additional subscriber that converts a handful of informative events
        /// to DAP "output" events. Without this, at best these would go to stderr, but vscode's
        /// executable DAP client ignores stderr, so this subscriber allows us to get that information
        /// into somewhere visible to the user.
        struct ConvertToDap;

        impl ConvertToDap {
            fn write_console(&self, msg: &str) -> buck2_error::Result<()> {
                let ev = debugserver_types::OutputEvent {
                    type_: "event".to_owned(),
                    event: "output".to_owned(),
                    // All other events are being sent by the debug support in the server and that's
                    // maintaining the sequence numbers. For us to get the correct sequence number
                    // here would be tricky. Instead, we just set it to 0 and hope that nobody notices/cares
                    // that it's out of order/invalid. The alternative would probably be to
                    // deserialize all events from the server and rewrite their sequence numbers (and
                    // potentially references to those sequence numbers coming back from the dap client).
                    seq: 0,
                    body: debugserver_types::OutputEventBody {
                        category: None,
                        column: None,
                        data: None,
                        line: None,
                        output: format!("{msg}\n"),
                        source: None,
                        variables_reference: None,
                    },
                };
                send_message_to_dap_client(&serde_json::to_vec(&ev)?)
            }
        }

        #[async_trait]
        impl EventSubscriber for ConvertToDap {
            async fn handle_output(&mut self, raw_output: &[u8]) -> buck2_error::Result<()> {
                Ok(self.write_console(&String::from_utf8_lossy(raw_output))?)
            }

            async fn handle_tailer_stderr(&mut self, stderr: &str) -> buck2_error::Result<()> {
                Ok(self.write_console(stderr)?)
            }

            async fn handle_events(
                &mut self,
                events: &[std::sync::Arc<BuckEvent>],
            ) -> buck2_error::Result<()> {
                for ev in events {
                    if let UnpackedBuckEvent::Instant(_, _, data) = unpack_event(ev)? {
                        match data {
                            buck2_data::instant_event::Data::StructuredError(soft_error) => {
                                if !soft_error.quiet {
                                    self.write_console(&format!(
                                        "soft error: {}",
                                        soft_error.payload
                                    ))?;
                                }
                            }
                            buck2_data::instant_event::Data::ConsoleMessage(message) => {
                                self.write_console(&message.message)?;
                            }
                            _ => {}
                        }
                    }
                }
                Ok(())
            }

            async fn handle_error(
                &mut self,
                error: &buck2_error::Error,
            ) -> buck2_error::Result<()> {
                Ok(self
                    .write_console(&format!("buck2 starlark-attach debugserver error: {error}"))?)
            }
        }

        vec![Box::new(ConvertToDap)]
    }
}

struct DapPartialResultHandler;

#[async_trait]
impl PartialResultHandler for DapPartialResultHandler {
    type PartialResult = buck2_cli_proto::DapMessage;

    async fn handle_partial_result(
        &mut self,
        mut _ctx: PartialResultCtx<'_>,
        partial_res: buck2_cli_proto::DapMessage,
    ) -> buck2_error::Result<()> {
        Ok(send_message_to_dap_client(&partial_res.dap_json)?)
    }
}
