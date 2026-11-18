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
use std::sync::LazyLock;

use buck2_core::error::SoftErrorContext;
use buck2_event_observer::dice_state::DiceState;
use buck2_event_observer::pending_estimate::pending_estimate;
use buck2_event_observer::span_tracker;
use buck2_event_observer::span_tracker::RootData;
use buck2_event_observer::span_tracker::Roots;
use buck2_events::BuckEvent;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::span::SpanId;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_wrapper_common::invocation_id::TraceId;
use dupe::Dupe;
use parking_lot::Mutex;
use parking_lot::MutexGuard;
use tokio::sync::oneshot;

static ACTIVE_COMMANDS: LazyLock<Mutex<BuckMutMap<TraceId, ActiveCommandHandle>>> =
    LazyLock::new(|| Mutex::new(BuckMutMap::default()));

/// Return the active commands, if you can access them.
pub fn try_active_commands() -> Option<BuckMutMap<TraceId, ActiveCommandHandle>> {
    // Note that this function is accessed during panic, so have to be super careful
    Some(ACTIVE_COMMANDS.try_lock()?.clone())
}

pub fn active_commands() -> MutexGuard<'static, BuckMutMap<TraceId, ActiveCommandHandle>> {
    ACTIVE_COMMANDS.lock()
}

/// A point-in-time snapshot of every active command's identity and progress.
pub fn active_commands_snapshot() -> Vec<buck2_subscription_proto::ActiveCommand> {
    active_commands()
        .iter()
        .map(|(trace_id, handle)| {
            let state = handle.state();
            let spans = state.spans();

            buck2_subscription_proto::ActiveCommand {
                trace_id: trace_id.to_string(),
                argv: state.argv.clone(),
                stats: Some(buck2_subscription_proto::ActiveCommandStats {
                    open_spans: spans.open,
                    closed_spans: spans.closed,
                    pending_spans: spans.pending,
                }),
            }
        })
        .collect()
}

/// Whether `trace_id` is the one and only active command. False if it isn't
/// registered (e.g. an empty map), so callers must hold their own active-command
/// guard when relying on this.
pub fn is_only_active_command(trace_id: &TraceId) -> bool {
    let active = ACTIVE_COMMANDS.lock();
    active.len() == 1 && active.contains_key(trace_id)
}

/// Broadcasts an instant event, returns whether any subscribers were connected.
pub fn broadcast_instant_event<E: Into<buck2_data::instant_event::Data> + Clone>(
    event: &E,
) -> bool {
    let mut has_subscribers = false;

    for cmd in ACTIVE_COMMANDS.lock().values() {
        cmd.dispatcher.instant_event(event.clone());
        has_subscribers = true;
    }

    has_subscribers
}

/// Sends an event only to the active command that owns `context`.
pub fn dispatch_soft_error_for_context<E: Into<buck2_data::instant_event::Data> + Clone>(
    context: &Arc<SoftErrorContext>,
    event: &E,
) -> bool {
    let active = ACTIVE_COMMANDS.lock();
    let Some(command) = active.values().find(|command| {
        command
            .dispatcher
            .soft_error_context()
            .is_some_and(|candidate| Arc::ptr_eq(&candidate, context))
    }) else {
        return false;
    };

    command.dispatcher.instant_event(event.clone());
    true
}

pub fn broadcast_shutdown(shutdown: &buck2_data::DaemonShutdown) {
    for cmd in ACTIVE_COMMANDS.lock().values() {
        cmd.notify_shutdown(shutdown.clone());
    }
}

/// Allows interactions with commands found via active_commands().
#[derive(Clone, Dupe)]
pub struct ActiveCommandHandle {
    /// A channel to send notifications to this command.
    dispatcher: EventDispatcher,

    /// A separate channel to broadcast shutdown events. This is separate from the EventDispatcher
    /// because we want to allow shutdown events to jump the queue.
    daemon_shutdown_channel: Arc<Mutex<Option<oneshot::Sender<buck2_data::DaemonShutdown>>>>,

    /// State for this command. This is used to expose what this command is doing to other clients.
    state: Arc<ActiveCommandState>,
}

impl ActiveCommandHandle {
    fn notify_shutdown(&self, shutdown: buck2_data::DaemonShutdown) {
        let channel = self.daemon_shutdown_channel.lock().take();

        if let Some(channel) = channel {
            let _ignored = channel.send(shutdown); // Nothing to do if receiver hung up.
        }
    }

    pub fn state(&self) -> &ActiveCommandState {
        self.state.as_ref()
    }
}

pub struct ActiveCommandDropGuard {
    trace_id: TraceId,
}

impl Drop for ActiveCommandDropGuard {
    fn drop(&mut self) {
        ACTIVE_COMMANDS.lock().remove(&self.trace_id);
    }
}

/// A handle to the stats for this command. We use this to broadcast state about this command.
pub struct ActiveCommandState {
    pub argv: Vec<String>,

    spans: Mutex<SpansSnapshot>,
}

impl ActiveCommandState {
    pub fn spans(&self) -> SpansSnapshot {
        *self.spans.lock()
    }

    fn new(argv: Vec<String>) -> Self {
        Self {
            argv,
            spans: Mutex::new(SpansSnapshot::default()),
        }
    }
}

#[derive(PartialEq, Debug, Default, Copy, Clone, Dupe)]
pub struct SpansSnapshot {
    pub open: u64,
    pub closed: u64,
    pub pending: u64,
}

/// A wrapper around ActiveCommandState that allows 1 client to write to it.
pub struct ActiveCommandStateWriter {
    /// Maps a SpanId to whether it is a root (i.e. no parent)
    roots: Roots<Arc<BuckEvent>>,
    non_roots: BuckMutSet<SpanId>,
    dice_state: DiceState,
    closed: u64,
    shared: Arc<ActiveCommandState>,
}

impl ActiveCommandStateWriter {
    fn new(shared: Arc<ActiveCommandState>) -> Self {
        Self {
            roots: Roots::default(),
            non_roots: BuckMutSet::default(),
            dice_state: DiceState::new(),
            closed: 0,
            shared,
        }
    }

    pub fn peek_event(&mut self, buck_event: &BuckEvent) {
        use buck2_data::buck_event::Data::*;

        let mut changed = false;

        match buck_event.data() {
            SpanStart(..) => {
                let span_id = match buck_event.span_id() {
                    Some(id) => id,
                    None => return,
                };

                if !span_tracker::is_span_shown(buck_event) {
                    return;
                }

                let is_root = buck_event
                    .parent_id()
                    .is_none_or(|id| !self.roots.contains(id) && !self.non_roots.contains(&id));

                if is_root {
                    self.roots.insert(span_id, false, RootData::new(buck_event));
                    changed = true;
                } else {
                    self.non_roots.insert(span_id);
                }
            }
            SpanEnd(..) => {
                let span_id = match buck_event.span_id() {
                    Some(id) => id,
                    None => return,
                };

                // If it's a root, then we increment closed.
                if self.roots.remove(span_id).is_some() {
                    self.closed += 1;
                    changed = true;
                } else {
                    self.non_roots.remove(&span_id);
                }
            }
            Instant(instant) => {
                use buck2_data::instant_event::Data::*;

                if let Some(DiceStateSnapshot(snapshot)) = instant.data.as_ref() {
                    self.dice_state.update(snapshot);
                    changed = true;
                }
            }
            _ => {}
        }

        if changed {
            let open = self.roots.len() as u64;
            let pending = pending_estimate(&self.roots, &self.dice_state);

            *self.shared.spans.lock() = SpansSnapshot {
                open,
                closed: self.closed,
                pending,
            };
        }
    }
}

pub struct ActiveCommand {
    pub guard: ActiveCommandDropGuard,
    pub state: ActiveCommandStateWriter,
    pub daemon_shutdown_channel: oneshot::Receiver<buck2_data::DaemonShutdown>,
}

impl ActiveCommand {
    pub fn new(event_dispatcher: &EventDispatcher, sanitized_argv: Vec<String>) -> Self {
        let (sender, receiver) = oneshot::channel();

        let state = Arc::new(ActiveCommandState::new(sanitized_argv));

        let trace_id = event_dispatcher.trace_id().dupe();
        let result = {
            // Scope the guard so it's locked as little as possible
            let mut active_commands = ACTIVE_COMMANDS.lock();

            let existing_active_commands = if active_commands.is_empty() {
                None
            } else {
                Some(active_commands.clone())
            };

            active_commands.insert(
                trace_id.dupe(),
                ActiveCommandHandle {
                    dispatcher: event_dispatcher.dupe(),
                    daemon_shutdown_channel: Arc::new(Mutex::new(Some(sender))),
                    state: state.dupe(),
                },
            );

            existing_active_commands
        };

        if let Some(commands) = result {
            // Notify our command it is running concurrently with others.
            event_dispatcher.instant_event(buck2_data::ConcurrentCommands {
                trace_ids: commands.keys().map(|cmd| cmd.to_string()).collect(),
            });

            // Notify other commands that they are concurrent with ours.
            for cmd in commands.values() {
                cmd.dispatcher
                    .instant_event(buck2_data::ConcurrentCommands {
                        trace_ids: vec![trace_id.to_string()],
                    });
            }
        }

        Self {
            guard: ActiveCommandDropGuard { trace_id },
            daemon_shutdown_channel: receiver,
            state: ActiveCommandStateWriter::new(state),
        }
    }
}

/// `cargo test` runs the tests of this crate on threads of one process, which has one registry
/// of active commands. A test that registers a command holds this until the command is gone.
#[cfg(test)]
pub(crate) static REGISTRY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use assert_matches::assert_matches;
    use buck2_events::Event;
    use buck2_events::daemon_id::DaemonId;
    use buck2_events::source::ChannelEventSource;
    use buck2_hash::IntentionallyStdHashMap;

    use super::*;

    #[test]
    fn test_active_command_state() {
        let mut writer =
            ActiveCommandStateWriter::new(Arc::new(ActiveCommandState::new(Vec::new())));

        let root = SpanId::next();
        let child = SpanId::next();
        let trace = TraceId::new();

        writer.peek_event(&BuckEvent::new(
            SystemTime::now(),
            trace.clone(),
            Some(root),
            None,
            buck2_data::SpanStartEvent {
                data: Some(buck2_data::AnalysisStart::default().into()),
            }
            .into(),
        ));

        assert_eq!(
            writer.shared.spans(),
            SpansSnapshot {
                open: 1,
                closed: 0,
                pending: 0
            }
        );

        writer.peek_event(&BuckEvent::new(
            SystemTime::now(),
            trace.clone(),
            Some(child),
            Some(root),
            buck2_data::SpanStartEvent {
                data: Some(buck2_data::AnalysisStageStart::default().into()),
            }
            .into(),
        ));

        assert_eq!(
            writer.shared.spans(),
            SpansSnapshot {
                open: 1,
                closed: 0,
                pending: 0
            }
        );

        writer.peek_event(&BuckEvent::new(
            SystemTime::now(),
            trace.clone(),
            Some(child),
            Some(root),
            buck2_data::SpanEndEvent {
                data: Some(buck2_data::AnalysisStageEnd::default().into()),
                ..Default::default()
            }
            .into(),
        ));

        assert_eq!(
            writer.shared.spans(),
            SpansSnapshot {
                open: 1,
                closed: 0,
                pending: 0
            }
        );

        writer.peek_event(&BuckEvent::new(
            SystemTime::now(),
            trace.clone(),
            Some(root),
            None,
            buck2_data::SpanEndEvent {
                data: Some(buck2_data::AnalysisEnd::default().into()),
                ..Default::default()
            }
            .into(),
        ));

        assert_eq!(
            writer.shared.spans(),
            SpansSnapshot {
                open: 0,
                closed: 1,
                pending: 0
            }
        );

        writer.peek_event(&BuckEvent::new(
            SystemTime::now(),
            trace,
            None,
            None,
            buck2_data::InstantEvent {
                data: Some(
                    buck2_data::DiceStateSnapshot {
                        key_states: {
                            let mut map = IntentionallyStdHashMap::new();
                            map.insert(
                                "BuildKey".to_owned(),
                                buck2_data::DiceKeyState {
                                    started: 4,
                                    finished: 2,
                                    check_deps_started: 0,
                                    check_deps_finished: 0,
                                    compute_started: 0,
                                    compute_finished: 0,
                                },
                            );
                            map
                        },
                        core_state_queue_depth: 0,
                        core_state_processed_requests: 0,
                    }
                    .into(),
                ),
            }
            .into(),
        ));

        assert_eq!(
            writer.shared.spans(),
            SpansSnapshot {
                open: 0,
                closed: 1,
                pending: 2
            }
        );
    }

    fn create_dispatcher() -> (EventDispatcher, ChannelEventSource, TraceId) {
        let (daemon_dispatcher_events, daemon_dispatcher_sink) =
            buck2_events::create_source_sink_pair();
        let trace_id = TraceId::new();
        let dispatcher =
            EventDispatcher::new(trace_id.dupe(), DaemonId::new(), daemon_dispatcher_sink);

        (dispatcher, daemon_dispatcher_events, trace_id)
    }

    fn check_concurrent_command_trace_ids_eq(event: Option<Event>, expected_trace_ids: &[String]) {
        assert_matches!(event, Some(Event::Buck(event)) => {
            assert_matches!(
                event.data(),
                buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ConcurrentCommands(
                        buck2_data::ConcurrentCommands {
                            trace_ids,
                        }
                    ))
                }) => {
                    // Use HashSets because  trace ids may not be reported in the same order that we specified.
                    let trace_ids: BuckMutSet<&String> = trace_ids.iter().collect();
                    let expected_trace_ids: BuckMutSet<&String> = expected_trace_ids.iter().collect();
                    assert_eq!(trace_ids, expected_trace_ids);
                }
            );
        });
    }

    #[test]
    fn test_multiple_active_commands() {
        let _guard = REGISTRY_TEST_LOCK.blocking_lock();
        let (dispatcher1, mut source1, id1) = create_dispatcher();
        let _active1 = ActiveCommand::new(&dispatcher1, Vec::new());

        let (dispatcher2, mut source2, id2) = create_dispatcher();
        let _active2 = ActiveCommand::new(&dispatcher2, Vec::new());

        check_concurrent_command_trace_ids_eq(source1.try_receive(), &[id2.to_string()]);
        check_concurrent_command_trace_ids_eq(source2.try_receive(), &[id1.to_string()]);

        let (dispatcher3, mut source3, id3) = create_dispatcher();
        let _active3 = ActiveCommand::new(&dispatcher3, Vec::new());

        check_concurrent_command_trace_ids_eq(source1.try_receive(), &[id3.to_string()]);
        check_concurrent_command_trace_ids_eq(source2.try_receive(), &[id3.to_string()]);
        check_concurrent_command_trace_ids_eq(
            source3.try_receive(),
            &[id1.to_string(), id2.to_string()],
        );
    }

    #[test]
    fn soft_error_context_routes_to_owning_command() -> buck2_error::Result<()> {
        let _guard = REGISTRY_TEST_LOCK.blocking_lock();
        let first_context = Arc::new(SoftErrorContext::new("", "")?);
        let second_context = Arc::new(SoftErrorContext::new("", "")?);

        let (first_dispatcher, mut first_source, _) = create_dispatcher();
        let first_dispatcher = first_dispatcher.with_soft_error_context(first_context.dupe());
        let _first = ActiveCommand::new(&first_dispatcher, Vec::new());

        let (second_dispatcher, mut second_source, _) = create_dispatcher();
        let second_dispatcher = second_dispatcher.with_soft_error_context(second_context);
        let _second = ActiveCommand::new(&second_dispatcher, Vec::new());

        first_source.try_receive();
        second_source.try_receive();

        assert!(dispatch_soft_error_for_context(
            &first_context,
            &buck2_data::ConsoleMessage {
                message: "first only".to_owned(),
            }
        ));
        assert!(first_source.try_receive().is_some());
        assert!(second_source.try_receive().is_none());

        Ok(())
    }

    #[test]
    fn soft_error_context_does_not_route_after_owner_finishes() -> buck2_error::Result<()> {
        let _guard = REGISTRY_TEST_LOCK.blocking_lock();
        let context = Arc::new(SoftErrorContext::new("", "")?);

        let (owner_dispatcher, mut owner_source, _) = create_dispatcher();
        let owner_dispatcher = owner_dispatcher.with_soft_error_context(context.dupe());
        let owner = ActiveCommand::new(&owner_dispatcher, Vec::new());

        let (other_dispatcher, mut other_source, _) = create_dispatcher();
        let _other = ActiveCommand::new(&other_dispatcher, Vec::new());

        owner_source.try_receive();
        other_source.try_receive();
        drop(owner);

        assert!(!dispatch_soft_error_for_context(
            &context,
            &buck2_data::ConsoleMessage {
                message: "no owner".to_owned(),
            }
        ));
        assert!(other_source.try_receive().is_none());

        Ok(())
    }
}
