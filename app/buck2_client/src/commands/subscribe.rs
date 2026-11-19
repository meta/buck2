/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use buck2_cli_proto::protobuf_util::ProtobufSplitter;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::command_outcome::CommandOutcome;
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
use buck2_client_ctx::stream_util::reborrow_stream_for_static;
use buck2_client_ctx::streaming::StreamingCommand;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_subscription_proto::SubscriptionRequest;
use futures::stream::StreamExt;
use futures::stream::TryStreamExt;
use prost::Message;
use tokio_util::codec::FramedRead;

/// Open a subscription channel to the Buck2 daemon. This allows you to interact with the Buck2
/// daemon via the `stdin` and `stdout` of this command: you send requests to the daemon by writing
/// to `stdin`, and you get responses via `stdout`.
///
/// The protocol used by this command is length-prefixed protobuf. This format is a repeated series
/// of a varint followed by a record of the length indicated by said varint.
///
/// The protobuf spec for those records is described in
/// `buck2_subscription_proto/subscription.proto`. The client writes `SubscriptionRequest` and
/// reads `SubscriptionResponse`. See the documentation in `subscription.proto` to discover
/// available APIs.
///
/// This API does not (currently) allow invalid requests and will error out when one is sent.
#[derive(Debug, clap::Parser)]
#[clap(about = "Subscribe to updates from the Buck2 daemon")]
pub struct SubscribeCommand {
    /// Whether to request command snapshots.
    #[clap(long)]
    active_commands: bool,

    /// Whether to get output as JSON. The JSON format is deemed unstable so this should only be
    /// used for debugging.
    #[clap(long)]
    unstable_json: bool,

    #[clap(flatten)]
    config_opts: CommonBuildConfigurationOptions,

    #[clap(flatten)]
    starlark_opts: CommonStarlarkOptions,

    #[clap(flatten)]
    event_log_opts: CommonEventLogOptions,
}

#[async_trait(?Send)]
impl StreamingCommand for SubscribeCommand {
    const COMMAND_NAME: &'static str = "subscribe";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let client_context = ctx.client_context(matches, &self)?;

        let stream = FramedRead::new(ctx.stdin(), ProtobufSplitter)
            .and_then(|bytes| {
                futures::future::ready(
                    SubscriptionRequest::decode_length_delimited(bytes)
                        .buck_error_context("Error decoding SubscriptionRequest"),
                )
            })
            .map(|res| match res {
                Ok(r) => r,
                Err(e) => {
                    // NOTE: if stderr is gone there is not much we can do besides not write to
                    // stderr.
                    let reason = format!("Error parsing request: {e:#}");
                    let _ignored = buck2_client_ctx::eprintln!("{}", reason);
                    SubscriptionRequest {
                        request: Some(
                            buck2_subscription_proto::Disconnect { reason, ok: false }.into(),
                        ),
                    }
                }
            });

        let mut partial_result_handler = SubscriptionPartialResultHandler {
            buffer: Vec::new(),
            json: self.unstable_json,
            ok: true,
        };

        let stream = if self.active_commands {
            futures::stream::once(futures::future::ready(SubscriptionRequest {
                request: Some(buck2_subscription_proto::SubscribeToActiveCommands {}.into()),
            }))
            .chain(stream)
            .left_stream()
        } else {
            stream.right_stream()
        };

        let stream = stream.map(|request| buck2_cli_proto::SubscriptionRequestWrapper {
            request: Some(request),
        });

        {
            let partial_result_handler = &mut partial_result_handler;

            reborrow_stream_for_static(
                stream,
                |stream| async move {
                    buckd
                        .with_flushing()
                        .subscription(client_context, stream, events_ctx, partial_result_handler)
                        .await
                },
                || {
                    Some(buck2_cli_proto::SubscriptionRequestWrapper {
                        request: Some(SubscriptionRequest {
                            request: Some(
                                buck2_subscription_proto::Disconnect {
                                    reason: "EOF on stdin".to_owned(),
                                    ok: true,
                                }
                                .into(),
                            ),
                        }),
                    })
                },
                || {
                    Some((
                        Duration::from_secs(5),
                        Ok(CommandOutcome::Success(
                            buck2_cli_proto::SubscriptionCommandResponse {},
                        )),
                    ))
                },
            )
            .await??;
        };

        if partial_result_handler.ok {
            ExitResult::success()
        } else {
            // FIXME(JakobDegen): This command should propagate some error information back from the
            // server so that we can do error handling here.
            buck2_error::buck2_error!(buck2_error::ErrorTag::Tier0, "Subscribe command failed")
                .into()
        }
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
        // It's normal to get no open spans for this command
        false
    }
}

/// Outputs subscription messages.
struct SubscriptionPartialResultHandler {
    /// We reuse our output buffer here.
    buffer: Vec<u8>,
    json: bool,
    ok: bool,
}

#[async_trait]
impl PartialResultHandler for SubscriptionPartialResultHandler {
    type PartialResult = buck2_cli_proto::SubscriptionResponseWrapper;

    async fn handle_partial_result(
        &mut self,
        mut ctx: PartialResultCtx<'_>,
        partial_res: Self::PartialResult,
    ) -> buck2_error::Result<()> {
        let response = partial_res
            .response
            .internal_error("Empty `SubscriptionResponseWrapper`")?;

        if let Some(buck2_subscription_proto::subscription_response::Response::Goodbye(goodbye)) =
            &response.response
        {
            self.ok = self.ok && goodbye.ok;
        }

        self.buffer.clear();

        if self.json {
            serde_json::to_writer(&mut self.buffer, &response)
                .buck_error_context("JSON encoding failed")?;
            self.buffer.push(b'\n');
        } else {
            response
                .encode_length_delimited(&mut self.buffer)
                .buck_error_context("Encoding failed")?;
        }

        ctx.stdout(&self.buffer).await
    }
}
