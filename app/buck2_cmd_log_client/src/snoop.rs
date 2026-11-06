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
use std::sync::Mutex;
use std::time::Duration;
use std::time::SystemTime;

use buck2_cli_proto::ClientContext;
use buck2_cli_proto::SubscriptionRequestWrapper;
use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::command_outcome::CommandOutcome;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::common::ui::get_console_with_root;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::NoPartialResultHandler;
use buck2_client_ctx::daemon::client::connect::BuckdConnectOptions;
use buck2_client_ctx::daemon::client::connect::connect_buckd;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::DaemonEventsCtx;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::events_ctx::PartialResultCtx;
use buck2_client_ctx::events_ctx::PartialResultHandler;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::signal_handler::with_simple_sigint_handler;
use buck2_client_ctx::subscribers::superconsole::timekeeper::Clock;
use buck2_client_ctx::subscribers::superconsole::timekeeper::RealtimeClock;
use buck2_client_ctx::subscribers::superconsole::timekeeper::SpeedKeyDescriptions;
use buck2_client_ctx::subscribers::superconsole::timekeeper::Timekeeper;
use buck2_client_ctx::ticker::Tick;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_error::ErrorTag;
use buck2_event_log::file_names::get_local_logs;
use buck2_event_log::read::EventLogPathBuf;
use buck2_event_log::tail::TailOptions;
use buck2_event_log::tail::WriterState;
use buck2_event_observer::span_tracker::EventTimestamp;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_subscription_proto::SubscriptionRequest;
use futures::StreamExt;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::style::Stylize;
use tokio::pin;
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::replay::ReplayResult;

/// Watch the progress of in-flight commands from another terminal.
///
/// Tails the event log of a command that is currently running (by default the newest
/// running command in this isolation dir) and renders it in a Superconsole, catching up on
/// past events and then following along live. Event content comes from the event log file;
/// if a daemon is already running, a read-only subscription to it additionally reports
/// which commands are live. Snoop never starts a daemon and does not interfere with the
/// commands being watched.
///
/// While snooping, the replay speed keys switch between commands instead: `k` moves to a
/// newer invocation and `j` to an older one — among the daemon's running commands when that
/// information is available, otherwise among the retained event logs, which includes
/// recently finished invocations (attaching to one replays it). Unless a specific
/// invocation was requested, snoop follows the daemon's view of what is running: it waits
/// for a command to start when nothing is running, and keeps picking up whatever command
/// runs next after the current one ends, until interrupted.
#[derive(Debug, clap::Parser)]
pub struct SnoopCommand {
    #[clap(flatten)]
    event_log: EventLogOptions,

    /// How many seconds to keep waiting for the current event log to grow again before
    /// treating its command as gone (e.g. the log was deleted, or the command exited
    /// without finishing its log). When the daemon reports the command is no longer
    /// running, the wait is cut short.
    #[clap(long, value_name = "SECONDS", default_value = "15")]
    idle_timeout: u64,

    #[clap(flatten)]
    console_opts: CommonConsoleOptions,
}

impl BuckSubcommand for SnoopCommand {
    const COMMAND_NAME: &'static str = "log-snoop";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        mut ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let Self {
            event_log,
            idle_timeout,
            console_opts,
        } = self;
        let idle_timeout = Duration::from_secs(idle_timeout);

        // With an explicit invocation selector, exit when that command ends; by default
        // keep picking up whatever command runs next.
        let follow_new =
            event_log.path.is_none() && event_log.trace_id.is_none() && event_log.recent.is_none();

        let live: LiveCommands = Arc::new(Mutex::new(None));
        let client_context = ctx.empty_client_context("log-snoop")?;
        let paths = ctx.paths()?.clone();
        // Only watch a daemon that already exists; snooping must not spawn one.
        let buckd = connect_buckd(BuckdConnectOptions::ExistingOnly, events_ctx, &paths)
            .await
            .ok();
        let daemon_available = buckd.is_some();
        let subscription =
            subscribe_to_active_commands(buckd, paths, client_context, events_ctx, live.clone());
        pin!(subscription);

        if daemon_available {
            // Drive the subscription briefly so the initial console header can already
            // show what is running.
            tokio::select! {
                () = &mut subscription => {}
                () = wait_for_first_snapshot(&live) => {}
            }
        }

        let snoop = async {
            let log_dir = ctx.paths()?.log_dir();
            let mut current = if follow_new {
                // Auto-follow attaches only through the daemon's active-commands info;
                // event logs on disk never pick the target, since a log without a
                // running command is a finished command and attaching to it would
                // replay it rather than follow anything.
                let newest_running = running_logs(&log_dir, &live)?
                    .and_then(|running| running.into_iter().next_back());
                match newest_running {
                    Some(log) => log,
                    None => {
                        buck2_client_ctx::eprintln!(
                            "No command is running. Waiting for one to start (Ctrl-C to exit)..."
                        )?;
                        wait_for_running(&log_dir, None, &live).await?
                    }
                }
            } else {
                event_log.get(&ctx).await?
            };
            loop {
                match snoop_one(&mut ctx, &console_opts, &current, idle_timeout, &live).await? {
                    SnoopOutcome::Switch(target) => current = target,
                    SnoopOutcome::Ended { invocation, res } => {
                        match res {
                            Ok(CommandOutcome::Success(result)) => {
                                for e in &result.errors {
                                    buck2_client_ctx::eprintln!("{}", e.message)?;
                                }
                            }
                            Ok(CommandOutcome::Failure(_)) => {
                                buck2_client_ctx::eprintln!("Command failed")?;
                            }
                            Err(e) if e.has_tag(ErrorTag::MissingCommandResult) => {
                                buck2_client_ctx::eprintln!(
                                    "The command ended without a result: it was likely interrupted, or its log was deleted."
                                )?;
                            }
                            Err(e) if e.has_tag(ErrorTag::InterruptedByDaemonShutdown) => {
                                buck2_client_ctx::eprintln!(
                                    "The buck2 daemon shut down while the command was running."
                                )?;
                            }
                            Err(e) => return ExitResult::from(e),
                        }
                        // The console's canvas (and the argv banner in it) is gone by
                        // now; restate what was being watched in plain scrollback.
                        buck2_client_ctx::eprintln!("Finished snooping {}", invocation)?;

                        if !follow_new {
                            return ExitResult::success();
                        }
                        buck2_client_ctx::eprintln!(
                            "Waiting for a new command to snoop (Ctrl-C to exit)..."
                        )?;
                        current = wait_for_running(&log_dir, Some(&current), &live).await?;
                    }
                }
            }
        };

        let work = async {
            tokio::select! {
                res = snoop => res,
                () = &mut subscription => unreachable!("subscription future never completes"),
            }
        };

        with_simple_sigint_handler(work)
            .await
            .unwrap_or_else(ExitResult::signal_interrupt)
    }
}

/// The daemon's currently running commands, or `None` when no daemon subscription is
/// available.
type LiveCommands = Arc<Mutex<Option<Vec<buck2_subscription_proto::ActiveCommand>>>>;

const LOCK_MSG: &str = "should not be poisoned: no code panics while holding this lock";

/// How often to retry connecting the daemon subscription while there is no daemon to
/// watch: one may appear later (e.g. spawned by the first command run after snoop
/// started) or restart.
const SUBSCRIPTION_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Keep `live` updated with the daemon's active commands. Never resolves. Connecting is
/// retried, so a daemon that appears or restarts while snoop runs is picked up; whenever
/// no subscription is up, `live` reverts to unknown.
async fn subscribe_to_active_commands(
    mut buckd: Option<BuckdClientConnector>,
    paths: InvocationPaths,
    client_context: ClientContext,
    events_ctx: &mut EventsCtx,
    live: LiveCommands,
) {
    loop {
        if let Some(mut buckd) = buckd.take() {
            subscription_impl(&mut buckd, client_context.clone(), &live).await;
            *live.lock().expect(LOCK_MSG) = None;
        }
        tokio::time::sleep(SUBSCRIPTION_RETRY_INTERVAL).await;
        // Only watch a daemon that already exists; snooping must not spawn one.
        buckd = connect_buckd(BuckdConnectOptions::ExistingOnly, events_ctx, &paths)
            .await
            .ok();
    }
}

async fn subscription_impl(
    buckd: &mut BuckdClientConnector,
    client_context: ClientContext,
    live: &LiveCommands,
) {
    let requests = futures::stream::once(futures::future::ready(SubscriptionRequestWrapper {
        request: Some(SubscriptionRequest {
            request: Some(buck2_subscription_proto::SubscribeToActiveCommands {}.into()),
        }),
    }))
    .chain(futures::stream::pending());

    let mut events_ctx = EventsCtx::new(None, Vec::new());
    let mut handler = LiveCommandsHandler {
        live: live.clone(),
        own_trace_id: client_context.trace_id.clone(),
    };
    let _ignored = buckd
        .with_flushing()
        .subscription(client_context, requests, &mut events_ctx, &mut handler)
        .await;
}

/// Snapshots arrive every ~100ms once the subscription is up; wait briefly for the first
/// one so the initial console header can already show what is running.
async fn wait_for_first_snapshot(live: &LiveCommands) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while live.lock().expect(LOCK_MSG).is_none() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Stores the most recent active commands snapshot from the daemon.
struct LiveCommandsHandler {
    live: LiveCommands,
    /// The daemon registers snoop's own subscription as an active command too; it is not
    /// something anyone can snoop, so drop it from the snapshots.
    own_trace_id: String,
}

#[async_trait::async_trait]
impl PartialResultHandler for LiveCommandsHandler {
    type PartialResult = buck2_cli_proto::SubscriptionResponseWrapper;

    async fn handle_partial_result(
        &mut self,
        _ctx: PartialResultCtx<'_>,
        partial_res: Self::PartialResult,
    ) -> buck2_error::Result<()> {
        use buck2_subscription_proto::subscription_response::Response;
        if let Some(Response::ActiveCommandsSnapshot(snapshot)) =
            partial_res.response.and_then(|r| r.response)
        {
            *self.live.lock().expect(LOCK_MSG) = Some(
                snapshot
                    .active_commands
                    .into_iter()
                    .filter(|c| c.trace_id != self.own_trace_id)
                    .collect(),
            );
        }
        Ok(())
    }
}

/// The event logs of the daemon's running commands, oldest first, or `None` when no
/// daemon subscription is available.
fn running_logs(
    log_dir: &AbsNormPath,
    live: &LiveCommands,
) -> buck2_error::Result<Option<Vec<EventLogPathBuf>>> {
    let Some(active) = live.lock().expect(LOCK_MSG).clone() else {
        return Ok(None);
    };
    let running: Vec<String> = active.into_iter().map(|c| c.trace_id).collect();
    let logs = get_local_logs(log_dir)?
        .into_iter()
        .filter(|log| {
            log.uuid_from_filename()
                .is_ok_and(|id| running.contains(&id.to_string()))
        })
        .collect();
    Ok(Some(logs))
}

enum SnoopOutcome {
    /// The user asked to snoop a different invocation.
    Switch(EventLogPathBuf),
    /// The event stream ended.
    Ended {
        /// `<trace_id>: <command line>` of the invocation that ended, for reporting
        /// after its console is gone.
        invocation: String,
        res: buck2_error::Result<CommandOutcome<ReplayResult>>,
    },
}

/// Snoop a single invocation until its stream ends or the user switches away.
async fn snoop_one(
    ctx: &mut ClientCommandContext<'_>,
    console_opts: &CommonConsoleOptions,
    log_path: &EventLogPathBuf,
    idle_timeout: Duration,
    live: &LiveCommands,
) -> buck2_error::Result<SnoopOutcome> {
    let log_dir = ctx.paths()?.log_dir();
    let isolation = ctx.paths()?.isolation.clone();

    let (writer_state_tx, writer_state_rx) = watch::channel(WriterState::Unknown);
    let (invocation, events) = log_path
        .unpack_stream_tailing(TailOptions {
            // The writer flushes the log on each of its ~100ms ticks.
            poll_interval: Duration::from_millis(100),
            idle_timeout: Some(idle_timeout),
            writer_state: Some(writer_state_rx),
        })
        .await?;

    // The header slot is too cramped for the command line (it shares a row with the
    // action stats), so show it as a banner line inside the canvas: switching
    // invocations then replaces it instead of accumulating scrollback. The banner is
    // styled (reverse-video chip + dimmed trace id) so the snoop chrome is visually
    // distinct from the snooped command's own superconsole output.
    let display_args = invocation.display_args();
    let invocation_desc = format!("{}: {}", invocation.trace_id, display_args);
    let banner = Lines(vec![Line::from_iter([
        Span::new_styled_lossy(format!(" SNOOP {isolation} ").bold().reverse()),
        Span::padding(1),
        Span::new_styled_lossy(invocation.trace_id.to_string().dim()),
        Span::sanitized(format!(": {display_args}")),
    ])]);

    let header = match running_logs(&log_dir, live)? {
        Some(running) => match running.iter().position(|l| l.path() == log_path.path()) {
            Some(i) => format!(
                "(snoop [{}] {}/{} running)",
                isolation,
                i + 1,
                running.len()
            ),
            None => format!(
                "(snoop [{}] finished, {} running)",
                isolation,
                running.len()
            ),
        },
        None => {
            let logs = get_local_logs(&log_dir)?;
            match logs.iter().position(|l| l.path() == log_path.path()) {
                Some(i) => format!("(snoop [{}] {}/{})", isolation, i + 1, logs.len()),
                None => format!("(snoop [{}])", isolation),
            }
        }
    };

    let trace_id = invocation.trace_id.to_string();
    let start_time = match invocation.start_time {
        Some(start_time) => start_time.into(),
        None => SystemTime::now().into(),
    };
    let (switch_requests, mut switch_rx) = mpsc::unbounded_channel();
    let timekeeper = Timekeeper::new(
        Box::new(SnoopClock {
            realtime: RealtimeClock,
            switch_requests,
        }),
        EventTimestamp(start_time),
    );

    let mut superconsole_config = console_opts.superconsole_config();
    superconsole_config.banner = Some(banner);
    // File-change notifications describe the snooped command's own state; emitted
    // above the canvas they would pile up as scrollback across switches.
    superconsole_config.hide_file_watcher_events = true;

    let (console, used_superconsole) = get_console_with_root(
        invocation.trace_id,
        console_opts.console_type,
        ctx.verbosity,
        true,
        timekeeper,
        &header,
        superconsole_config,
        None,
        None,
    );

    // Without a superconsole there is no canvas to carry the banner; print it the
    // ordinary way instead.
    if !used_superconsole {
        buck2_client_ctx::eprintln!("Snooping [{}] {}", isolation, invocation_desc)?;
    }

    let mut events_ctx = EventsCtx::new(None, vec![console]);
    let outcome = {
        let mut daemon_events_ctx = DaemonEventsCtx::without_tailers(&mut events_ctx);
        let mut partial_result_handler = NoPartialResultHandler;
        let unpack = daemon_events_ctx.unpack_stream::<_, ReplayResult, _>(
            &mut partial_result_handler,
            Box::pin(events),
            ctx.console_interaction_stream(console_opts),
        );
        pin!(unpack);

        let mut live_check = tokio::time::interval(Duration::from_millis(500));
        let mut absent_from_snapshot = 0u32;

        loop {
            tokio::select! {
                res = &mut unpack => {
                    break SnoopOutcome::Ended { invocation: invocation_desc, res };
                }
                _ = live_check.tick() => {
                    // Track the daemon's view of this command for the tail: running
                    // suppresses the idle timeout (a quiet command is not gone), absent
                    // means its writer is done, so the tail can end after a short grace
                    // instead of the full idle timeout. Absence must be seen in two
                    // consecutive snapshots (we may have attached before the command made
                    // it into one), and a command that appears again un-latches: a
                    // transient snapshot gap must not end a live tail.
                    let observed = match live.lock().expect(LOCK_MSG).as_ref() {
                        None => {
                            absent_from_snapshot = 0;
                            WriterState::Unknown
                        }
                        Some(active) if active.iter().any(|c| c.trace_id == trace_id) => {
                            absent_from_snapshot = 0;
                            WriterState::Running
                        }
                        Some(_) => {
                            absent_from_snapshot += 1;
                            if absent_from_snapshot >= 2 {
                                WriterState::Finished
                            } else {
                                *writer_state_tx.borrow()
                            }
                        }
                    };
                    if *writer_state_tx.borrow() != observed {
                        // Nothing to do if the tail already hung up.
                        let _ignored = writer_state_tx.send(observed);
                    }
                }
                Some(direction) = switch_rx.recv() => {
                    // Prefer navigating among the daemon's running commands; fall back to all
                    // retained logs when that information isn't available or we're viewing a
                    // finished invocation.
                    let (logs, running_only) = match running_logs(&log_dir, live)? {
                        Some(running) if running.iter().any(|l| l.path() == log_path.path()) => {
                            (running, true)
                        }
                        _ => (get_local_logs(&log_dir)?, false),
                    };
                    let position = logs.iter().position(|l| l.path() == log_path.path());
                    let target = match (direction, position) {
                        (SwitchDirection::Newer, Some(i)) => logs.get(i + 1),
                        (SwitchDirection::Older, Some(0)) => None,
                        (SwitchDirection::Older, Some(i)) => logs.get(i - 1),
                        // The log we were snooping was deleted; jump to the newest one.
                        (_, None) => logs.last().filter(|l| l.path() != log_path.path()),
                    };
                    match target {
                        Some(target) => break SnoopOutcome::Switch(target.clone()),
                        None => {
                            let which = match direction {
                                SwitchDirection::Newer => "newer",
                                SwitchDirection::Older => "older",
                            };
                            let what = if running_only { "running command" } else { "event log" };
                            buck2_client_ctx::eprintln!("No {} {} to snoop", which, what)?;
                        }
                    }
                }
            }
        }
    };

    if matches!(outcome, SnoopOutcome::Switch(_)) {
        // The canvas (and its banner) belong to the invocation being left behind;
        // erase them so the next console replaces the display instead of pushing
        // it into scrollback.
        events_ctx.erase_interactive_output().await?;
    }
    Ok(outcome)
}

/// Wait for the daemon to report a running command other than `current` (whose end
/// brought us here — snapshots can lag its end by a beat) and return its event log.
/// Waits indefinitely: with no daemon connection there is no daemon info, but also
/// nothing running, and the subscription retry picks up a daemon that appears later.
async fn wait_for_running(
    log_dir: &AbsNormPath,
    current: Option<&EventLogPathBuf>,
    live: &LiveCommands,
) -> buck2_error::Result<EventLogPathBuf> {
    loop {
        let candidate = running_logs(log_dir, live)?.and_then(|running| {
            running
                .into_iter()
                .rev()
                .find(|l| current.is_none_or(|c| c.path() != l.path()))
        });
        if let Some(candidate) = candidate {
            return Ok(candidate);
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[derive(Copy, Clone)]
enum SwitchDirection {
    Older,
    Newer,
}

/// A realtime clock that repurposes the replay speed keys: `k` (faster) asks the snoop
/// driver to switch to a newer command and `j` (slower) to an older one.
struct SnoopClock {
    realtime: RealtimeClock,
    switch_requests: mpsc::UnboundedSender<SwitchDirection>,
}

#[async_trait::async_trait]
impl Clock for SnoopClock {
    fn event_timestamp_for_tick(&mut self, tick: Tick) -> EventTimestamp {
        self.realtime.event_timestamp_for_tick(tick)
    }

    fn speed_key_descriptions(&self) -> SpeedKeyDescriptions {
        SpeedKeyDescriptions {
            increase: "snoop a newer invocation",
            decrease: "snoop an older invocation",
        }
    }

    async fn scale_speed(&mut self, factor: f64) -> Option<String> {
        let direction = if factor > 1.0 {
            SwitchDirection::Newer
        } else {
            SwitchDirection::Older
        };
        // The driver may already be tearing this console down; dropping the request is fine.
        let _ignored = self.switch_requests.send(direction);
        Some(
            match direction {
                SwitchDirection::Newer => "Looking for a newer command to snoop...",
                SwitchDirection::Older => "Looking for an older command to snoop...",
            }
            .to_owned(),
        )
    }
}
