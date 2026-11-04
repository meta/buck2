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
use std::sync::atomic::AtomicU64;

use async_trait::async_trait;
use buck2_agent_interactive::Advice;
use buck2_agent_interactive::Decision;
use buck2_common::argv::Argv;
use buck2_common::argv::SanitizedArgv;
use buck2_common::invocation_paths::DEFAULT_ISOLATION_DIR;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_error::ErrorTag;
use buck2_error::ExitCode;
use buck2_event_observer::span_tracker::EventTimestamp;
use dupe::Dupe;

use crate::client_ctx::BuckSubcommand;
use crate::client_ctx::ClientCommandContext;
use crate::common::BuckArgMatches;
use crate::common::CommonBuildConfigurationOptions;
use crate::common::CommonEventLogOptions;
use crate::common::CommonStarlarkOptions;
use crate::common::ui::CommonConsoleOptions;
use crate::common::ui::get_console_with_root;
use crate::daemon::client::BuckdClientConnector;
use crate::daemon::client::connect::BuckdConnectDaemonOptions;
use crate::daemon::client::connect::BuckdConnectOptions;
use crate::daemon::client::connect::DaemonConstraintsRequest;
use crate::daemon::client::connect::DaemonStartupMode;
use crate::daemon::client::connect::DesiredTraceIoState;
use crate::daemon::client::connect::connect_buckd;
use crate::events_ctx::EventsCtx;
use crate::exit_result::ExitResult;
use crate::path_arg::PathArg;
use crate::remote_sink_config;
use crate::signal_handler::with_simple_sigint_handler;
use crate::subscribers::build_graph_stats::BuildGraphStats;
use crate::subscribers::build_id_writer::BuildIdWriter;
use crate::subscribers::event_log::EventLog;
use crate::subscribers::health_check_subscriber::HealthCheckSubscriber;
use crate::subscribers::re_log::ReLog;
use crate::subscribers::subscriber::EventSubscriber;
use crate::subscribers::superconsole::timekeeper::RealtimeClock;
use crate::subscribers::superconsole::timekeeper::Timekeeper;
use crate::subscribers::test_id_writer::TestIdWriter;

const HEALTH_CHECK_CHANNEL_SIZE: usize = 100;

fn update_events_ctx<T: StreamingCommand>(
    cmd: &T,
    matches: BuckArgMatches<'_>,
    ctx: &ClientCommandContext,
    events_ctx: &mut EventsCtx,
) {
    let console_opts = cmd.console_opts();
    let event_log_opts = cmd.event_log_opts();
    let mut subscribers = vec![];
    let expect_spans = cmd.should_expect_spans();

    let paths = ctx.paths().ok();
    let bes_results_url = remote_sink_config::bes_results_url(paths);

    // Need this to get information from one subscriber (event_log)
    // and log it in another (invocation_recorder)
    let log_size_counter_bytes = Some(Arc::new(AtomicU64::new(0)));

    let enable_health_checks = ctx
        .buck_settings()
        .and_then(|settings| {
            let mut buckconfig = ctx.immediate_config.root_config()?;
            settings.invocation.health_check.enabled(&mut buckconfig)
        })
        .unwrap_or(false)
        // Force-enable health checks when the test override is set.
        || std::env::var_os("BUCK2_TEST_SLOW_BUILD_CHECK").is_some();

    let (
        health_check_tags_receiver,
        health_check_display_reports_receiver,
        health_check_subscriber,
    ) = if enable_health_checks {
        let (tag_tx, tag_rx) = tokio::sync::mpsc::channel(HEALTH_CHECK_CHANNEL_SIZE);
        let (report_tx, report_rx) = tokio::sync::mpsc::channel(HEALTH_CHECK_CHANNEL_SIZE);
        let subscriber = HealthCheckSubscriber::new(tag_tx, report_tx, paths);
        (Some(tag_rx), Some(report_rx), Some(subscriber))
    } else {
        (None, None, None)
    };

    let (console_subscriber, used_superconsole) = get_console_with_root(
        ctx.trace_id.dupe(),
        console_opts.console_type,
        ctx.verbosity,
        expect_spans,
        Timekeeper::new(
            Box::new(RealtimeClock),
            EventTimestamp(ctx.start_time.into()),
        ),
        T::COMMAND_NAME,
        console_opts.superconsole_config(),
        health_check_display_reports_receiver,
        bes_results_url,
    );
    subscribers.push(console_subscriber);
    events_ctx.used_superconsole = used_superconsole;

    if let Some(paths) = paths {
        let re_log_subscriber = ReLog::new(paths.isolation.clone());
        subscribers.push(Box::new(re_log_subscriber));

        if !event_log_opts.no_event_log {
            let event_log_subscriber =
                get_event_log_subscriber(cmd, ctx, log_size_counter_bytes.clone(), paths);
            subscribers.push(event_log_subscriber);
        }
    }
    if let Some(build_id_writer) = get_build_id_writer(cmd.event_log_opts(), ctx) {
        subscribers.push(build_id_writer)
    }
    if let Some(test_id_writer) = get_test_id_writer(cmd, ctx) {
        subscribers.push(test_id_writer)
    }
    if let Some(build_graph_stats) = get_build_graph_stats(cmd, ctx) {
        subscribers.push(build_graph_stats)
    }
    let representative_config_flags = if ctx.paths().is_ok() {
        matches.get_representative_config_flags()
    } else {
        Vec::new()
    };

    if let Some(recorder) = events_ctx.recorder.as_mut() {
        recorder.update_for_command(
            ctx,
            cmd.event_log_opts(),
            cmd.sanitize_argv(ctx.argv.clone()).argv,
            Some(cmd.build_config_opts()),
            representative_config_flags,
            log_size_counter_bytes,
            health_check_tags_receiver,
            paths,
        );
    }

    if let Some(subscriber) = health_check_subscriber {
        subscribers.push(subscriber);
    }

    subscribers.extend(cmd.extra_subscribers());
    events_ctx.subscribers = subscribers;
}

/// Trait to generalize the behavior of executable buck2 commands that rely on a server.
/// This trait is most helpful when the command wants a superconsole, to stream events, etc.
/// However, this is the most robustly tested of our code paths, and there is little cost to defaulting to it.
/// As a result, prefer to default to streaming mode unless there is a compelling reason not to
/// (e.g `status`)
#[async_trait(?Send)]
pub trait StreamingCommand: Sized + Send + Sync {
    const COMMAND_NAME: &'static str;

    /// Run the command.
    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult;

    /// Should we only connect to existing servers (`true`), or spawn a new server if required (`false`).
    /// Defaults to `false`.
    fn existing_only() -> bool {
        false
    }

    fn trace_io(&self) -> DesiredTraceIoState {
        DesiredTraceIoState::Existing
    }

    fn daemon_startup_mode() -> DaemonStartupMode {
        DaemonStartupMode::Standard
    }

    fn console_opts(&self) -> &CommonConsoleOptions;

    fn event_log_opts(&self) -> &CommonEventLogOptions;

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions;

    fn starlark_opts(&self) -> &CommonStarlarkOptions;

    fn extra_subscribers(&self) -> Vec<Box<dyn EventSubscriber>> {
        vec![]
    }

    fn sanitize_argv(&self, argv: Argv) -> SanitizedArgv {
        argv.no_need_to_sanitize()
    }

    /// Some commands should always be displaying
    /// at least 1 ongoing process in the terminal, aka "spans".
    /// In the simple console, we want to display a "Waiting for daemon..." message
    /// exclusively for these commands whenever there are long periods without spans.
    fn should_expect_spans(&self) -> bool {
        true
    }

    /// Currently only for BxlCommand.
    fn user_event_log(&self) -> &Option<PathArg> {
        &None
    }

    /// Path to write test session ID. Currently only for TestCommand.
    fn write_test_id(&self) -> &Option<PathArg> {
        &None
    }
}

impl<T: StreamingCommand> BuckSubcommand for T {
    const COMMAND_NAME: &'static str = T::COMMAND_NAME;

    /// Actual call that runs a `StreamingCommand`.
    /// Handles the business of setting up a server connection for streaming.
    async fn exec_impl(
        self,
        matches: BuckArgMatches<'_>,
        mut ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let work = async {
            let mut connect_options = if T::existing_only() {
                BuckdConnectOptions::ExistingOnly
            } else {
                let mut req =
                    DaemonConstraintsRequest::new(ctx.immediate_config, T::trace_io(&self))?;
                ctx.restarter.apply_to_constraints(&mut req);
                BuckdConnectOptions::Options(BuckdConnectDaemonOptions {
                    constraints: req,
                    daemon_startup_mode: T::daemon_startup_mode(),
                    new_isolation_dir_advice: if ctx.paths()?.isolation.as_str()
                        == DEFAULT_ISOLATION_DIR
                    {
                        Decision::Inactive
                    } else {
                        ctx.agent_advice(Advice::NewIsolationDir)?
                    },
                    trace_id: ctx.trace_id.dupe(),
                    #[cfg(all(fbcode_build, target_os = "linux"))]
                    allow_daemon_start_unsandboxed_via_wrapper: ctx
                        .immediate_config
                        .allow_daemon_start_unsandboxed_via_wrapper()?,
                })
            };
            let buckd = match ctx.start_in_process_daemon.take() {
                None => connect_buckd(connect_options, events_ctx, ctx.paths()?).await,
                Some(start_in_process_daemon) => {
                    // Start in-process daemon, wait until it is ready to accept connections.
                    start_in_process_daemon()?;

                    // Do not attempt to spawn a daemon if connect failed.
                    // Connect should not fail.
                    connect_options = BuckdConnectOptions::ExistingOnly;

                    connect_buckd(connect_options, events_ctx, ctx.paths()?).await
                }
            };

            let mut buckd = match buckd {
                Ok(buckd) => buckd,
                Err(e) if e.has_tag(ErrorTag::AgentAdviceBlocked) => return ExitResult::err(e),
                Err(e) => {
                    return ExitResult::err_with_exit_code(e, ExitCode::ConnectError);
                }
            };

            events_ctx.daemon_pid = Some(buckd.daemon_pid);
            events_ctx.cgroup_path_of_buck2_daemon = buckd.cgroup_path_of_buck2_daemon.clone();
            events_ctx.daemon_start_instant = buckd.daemon_start_instant;

            let command_result = self
                .exec_impl(&mut buckd, matches, &mut ctx, events_ctx)
                .await;

            ctx.restarter.observe(&buckd, events_ctx);

            command_result
        };

        // FIXME: move this into client_ctx
        with_simple_sigint_handler(work)
            .await
            .unwrap_or_else(ExitResult::signal_interrupt)
    }

    fn update_events_ctx(
        &self,
        matches: BuckArgMatches<'_>,
        ctx: &ClientCommandContext,
        events_ctx: &mut EventsCtx,
    ) {
        update_events_ctx(self, matches, ctx, events_ctx);
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        self.event_log_opts()
    }

    fn logging_name(&self) -> &'static str {
        Self::COMMAND_NAME
    }
}

/// Given the command arguments, conditionally create an event log.
fn get_event_log_subscriber<T: StreamingCommand>(
    cmd: &T,
    ctx: &ClientCommandContext,
    log_size_counter_bytes: Option<Arc<AtomicU64>>,
    paths: &InvocationPaths,
) -> Box<dyn EventSubscriber> {
    let event_log_opts = cmd.event_log_opts();
    let sanitized_argv = cmd.sanitize_argv(ctx.argv.clone());
    let user_event_log = cmd.user_event_log();

    let logdir = paths.log_dir();
    let log = EventLog::new(
        logdir,
        ctx.working_dir.clone(),
        event_log_opts
            .event_log
            .as_ref()
            .map(|p| p.resolve(&ctx.working_dir)),
        user_event_log.as_ref().map(|p| p.resolve(&ctx.working_dir)),
        sanitized_argv,
        T::COMMAND_NAME.to_owned(),
        ctx.start_time,
        log_size_counter_bytes,
        ctx.buck_settings()
            .map(|settings| settings.invocation.event_log.retained_logs())
            .unwrap_or_else(|_| buck2_settings::EventLog::default().retained_logs()),
    );
    Box::new(log)
}

fn get_build_id_writer(
    opts: &CommonEventLogOptions,
    ctx: &ClientCommandContext,
) -> Option<Box<dyn EventSubscriber>> {
    if let Some(file_loc) = opts.write_build_id.as_ref() {
        Some(Box::new(BuildIdWriter::new(
            file_loc.resolve(&ctx.working_dir),
        )))
    } else {
        None
    }
}

fn get_test_id_writer<T: StreamingCommand>(
    cmd: &T,
    ctx: &ClientCommandContext,
) -> Option<Box<dyn EventSubscriber>> {
    if let Some(file_loc) = cmd.write_test_id().as_ref() {
        Some(Box::new(TestIdWriter::new(
            file_loc.resolve(&ctx.working_dir),
        )))
    } else {
        None
    }
}

fn get_build_graph_stats<T: StreamingCommand>(
    cmd: &T,
    ctx: &ClientCommandContext,
) -> Option<Box<dyn EventSubscriber>> {
    if should_handle_build_graph_stats(cmd) {
        Some(Box::new(BuildGraphStats::new_with_paths(
            ctx.fbinit(),
            ctx.trace_id.dupe(),
            ctx.paths().ok(),
        )))
    } else {
        None
    }
}

fn should_handle_build_graph_stats<T: StreamingCommand>(cmd: &T) -> bool {
    // Currently, we only care about graph size info in BuildResponse which build command produces
    cmd.build_config_opts()
        .config_values
        .contains(&"buck2.log_configured_graph_size=true".to_owned())
        && cmd.logging_name() == "build"
}
