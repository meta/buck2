/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Discard decoded payload the ordered scanner never reads.
//!
//! This runs AFTER the ordinary decoder has validated the entire record and
//! BEFORE retained-byte accounting / cross-thread handoff. It does not avoid
//! prost's initial allocation, skip wire validation, filter records, or change
//! error ordering. Its purpose is to free unused allocations on the decoding
//! thread and avoid accounting / transferring them to the reducer.
//!
//! Instant retention and consumer dispatch share `InstantKind`; adding a kind
//! requires an explicit decision in each exhaustive scanner/profile match.
//! Unknown SpanStarts must remain SpanStarts for ancestry; recognized SpanEnd
//! kinds must never become the scanner's generic phase-ending fallback. All
//! action payloads and consumed instant payloads are deliberately unmodified
//! here. ActionKey identity must not be field-trimmed.

use buck2_event_log::stream_value::StreamValue;

/// The sole instant-payload selection table. Consumers match this enum without
/// wildcards, so a new kind cannot be consumed while compaction silently drops it.
/// This discriminator neither owns payloads nor changes timestamp requirements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InstantKind {
    ReSession,
    SystemInfo,
    Snapshot,
    BuildGraphInfo,
    TransitiveSetProjectionDef,
    ActionGraphStructureDef,
    ActionGraphAlias,
    ActionGraphStart,
    ActionGraphFooter,
    ConcurrentCommands,
    InstallFinished,
    Ignored,
}

pub(super) fn instant_kind(data: Option<&buck2_data::instant_event::Data>) -> InstantKind {
    use buck2_data::instant_event::Data;
    match data {
        Some(Data::ReSession(_)) => InstantKind::ReSession,
        Some(Data::SystemInfo(_)) => InstantKind::SystemInfo,
        Some(Data::Snapshot(_)) => InstantKind::Snapshot,
        Some(Data::BuildGraphInfo(_)) => InstantKind::BuildGraphInfo,
        Some(Data::TransitiveSetProjectionDef(_)) => InstantKind::TransitiveSetProjectionDef,
        Some(Data::ActionGraphStructureDef(_)) => InstantKind::ActionGraphStructureDef,
        Some(Data::ActionGraphAlias(_)) => InstantKind::ActionGraphAlias,
        Some(Data::ActionGraphStart(_)) => InstantKind::ActionGraphStart,
        Some(Data::ActionGraphFooter(_)) => InstantKind::ActionGraphFooter,
        Some(Data::ConcurrentCommands(_)) => InstantKind::ConcurrentCommands,
        Some(Data::InstallFinished(_)) => InstantKind::InstallFinished,
        _ => InstantKind::Ignored,
    }
}

/// Preserve one successful decoded record, removing only data currently unused
/// by `scan_with_phases(log, phases)`. Timestamp presence, IDs and duration
/// presence/value are unchanged, including malformed and zero values.
pub fn clean(mut value: StreamValue, _phases: bool) -> StreamValue {
    use buck2_data::buck_event::Data as Event;
    use buck2_data::span_end_event::Data as End;
    use buck2_data::span_start_event::Data as Start;

    match &mut value {
        StreamValue::Event(event) => {
            // `clear()` would retain the old allocation and its heap charge.
            // Invocation, not per-event trace_id, supplies the output run ID.
            event.trace_id = String::new();
            // Keep consumed kinds even without timestamps: the profile must
            // count malformed profile records, and capture schemas/identity
            // remain meaningful without positioned evidence. The reducer owns
            // the timestamp decision, not this allocation-only compaction.
            match &mut event.data {
                Some(Event::SpanStart(start)) => match &mut start.data {
                    Some(Start::ActionExecution(_))
                    | Some(Start::ExecutorStage(_))
                    | Some(Start::MatchDepFiles(_)) => {}
                    // Only the start discriminator is used for these two
                    // stages. Their END payloads are retained separately.
                    Some(Start::ReUpload(payload)) => *payload = Default::default(),
                    Some(Start::ReSubmit(payload)) => *payload = Default::default(),
                    Some(data) if super::profile::start_kind_kept(data) => {}
                    _ => start.data = None,
                },
                Some(Event::SpanEnd(end)) => {
                    end.stats = None;
                    // These are distinct scanner branches even when their
                    // payload happens to be empty. For example, converting
                    // ExecutorStage into None could wrongly close a phase
                    // that happens to reuse the same span ID.
                    if !end.data.as_ref().is_some_and(super::profile::end_kind_kept)
                        && !matches!(
                            &end.data,
                            Some(
                                End::ActionExecution(_)
                                    | End::Command(_)
                                    | End::FileWatcher(_)
                                    | End::ReUpload(_)
                                    | End::ReSubmit(_)
                                    | End::ExecutorStage(_)
                                    | End::MatchDepFiles(_)
                                    | End::SpanCancelled(_)
                            )
                        )
                    {
                        // Every other end takes the same phase fallback,
                        // including None, for both values of `phases`.
                        end.data = None;
                    }
                }
                Some(Event::Instant(instant)) => {
                    if instant_kind(instant.data.as_ref()) == InstantKind::Ignored {
                        instant.data = None;
                    }
                }
                _ => event.data = None,
            }
        }
        // Preserve terminal response variants so the collector owns their
        // interpretation; unclassified responses retain unknown outcome.
        StreamValue::Result(_) => {}
        StreamValue::PartialResult(result) => {
            // PartialResult still contributes to the successful-record count.
            result.partial_result = None;
        }
    }
    value
}

pub(super) fn clean_for_run(
    value: StreamValue,
    phases: bool,
    expected: Option<&[u8; 36]>,
) -> StreamValue {
    if let StreamValue::Event(event) = &value
        && let Some(expected) = expected
        && !event.trace_id.is_empty()
        && !event.trace_id.as_bytes().eq_ignore_ascii_case(expected)
    {
        // Preserve only the conflicting identity for the ordered reducer;
        // none of this event may join the invocation's action graph.
        let StreamValue::Event(mut event) = value else {
            unreachable!()
        };
        event.data = None;
        return StreamValue::Event(event);
    }
    clean(value, phases)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloned(value: &StreamValue) -> StreamValue {
        serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
    }

    fn event(data: buck2_data::buck_event::Data) -> StreamValue {
        let mut trace_id = String::with_capacity(256);
        trace_id.push_str("unused-per-event-trace-id");
        StreamValue::Event(Box::new(buck2_data::BuckEvent {
            timestamp: Some(prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos: 123_456_789,
            }),
            trace_id,
            span_id: u64::MAX,
            parent_id: 123,
            data: Some(data),
        }))
    }

    fn started(data: buck2_data::span_start_event::Data) -> StreamValue {
        event(buck2_data::buck_event::Data::SpanStart(
            buck2_data::SpanStartEvent { data: Some(data) },
        ))
    }

    fn ended(data: buck2_data::span_end_event::Data) -> StreamValue {
        event(buck2_data::buck_event::Data::SpanEnd(
            buck2_data::SpanEndEvent {
                data: Some(data),
                duration: Some(prost_types::Duration {
                    seconds: -1,
                    nanos: 7,
                }),
                stats: Some(buck2_data::SpanStats {
                    max_poll_time_us: 123,
                    total_poll_time_us: 456,
                }),
            },
        ))
    }

    fn instant(data: buck2_data::instant_event::Data) -> StreamValue {
        event(buck2_data::buck_event::Data::Instant(
            buck2_data::InstantEvent { data: Some(data) },
        ))
    }

    fn expected_outer_cleanup(mut value: StreamValue) -> StreamValue {
        if let StreamValue::Event(event) = &mut value {
            event.trace_id = String::new();
            if let Some(buck2_data::buck_event::Data::SpanEnd(end)) = &mut event.data {
                end.stats = None;
            }
        }
        value
    }

    fn assert_kept(value: StreamValue, phases: bool) {
        let expected = expected_outer_cleanup(cloned(&value));
        assert_eq!(
            serde_json::to_value(clean(value, phases)).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn ancestry_headers_survive_unused_starts_and_profile_kinds_are_always_kept() {
        use buck2_data::span_start_event::Data as Start;
        for phases in [false, true] {
            let original = started(Start::Fake(Default::default()));
            let StreamValue::Event(before) = cloned(&original) else {
                unreachable!()
            };
            let StreamValue::Event(after) = clean(original, phases) else {
                unreachable!()
            };
            assert_eq!(after.timestamp, before.timestamp);
            assert_eq!((after.span_id, after.parent_id), (u64::MAX, 123));
            assert_eq!(after.trace_id.capacity(), 0);
            let Some(buck2_data::buck_event::Data::SpanStart(start)) = after.data else {
                panic!("ancestry marker removed")
            };
            assert!(start.data.is_none());
        }
        for phase in [
            Start::Command(Default::default()),
            Start::CommandCritical(Default::default()),
            Start::FileWatcher(Default::default()),
            Start::ConnectToInstaller(Default::default()),
            Start::InstallEventInfo(Default::default()),
            Start::Load(Default::default()),
            Start::LoadPackage(Default::default()),
            Start::Analysis(Default::default()),
            Start::Materialization(Default::default()),
        ] {
            let value = started(phase);
            assert_kept(cloned(&value), true);
            assert_kept(value, false);
        }
    }

    #[test]
    fn actions_executor_starts_and_all_recognized_ends_keep_their_payloads() {
        use buck2_data::span_end_event::Data as End;
        use buck2_data::span_start_event::Data as Start;
        let action = started(Start::ActionExecution(buck2_data::ActionExecutionStart {
            key: Some(buck2_data::ActionKey {
                id: vec![1, 2, 3],
                key: "exact-key".to_owned(),
                ..Default::default()
            }),
            kind: 3,
            name: Some(buck2_data::ActionName {
                category: "rustc".to_owned(),
                identifier: "library".to_owned(),
            }),
        }));
        assert_kept(action, false);
        assert_kept(started(Start::ExecutorStage(Default::default())), false);
        for remote_cache in [false, true] {
            for checking_filtered_inputs in [false, true] {
                for phases in [false, true] {
                    assert_kept(
                        started(Start::MatchDepFiles(buck2_data::MatchDepFilesStart {
                            checking_filtered_inputs,
                            remote_cache,
                        })),
                        phases,
                    );
                }
            }
        }
        let mut action_end = End::ActionExecution(Default::default());
        if let End::ActionExecution(end) = &mut action_end {
            end.graph_producers = Some(buck2_data::ActionGraphProducers {
                span_ids: vec![91],
                tset_ids: vec![17],
                ..Default::default()
            });
            end.commands.push(buck2_data::CommandExecution {
                details: Some(buck2_data::CommandExecutionDetails {
                    // Deliberately unchanged even though the scanner does not
                    // read these: action trimming is a separate experiment.
                    cmd_stdout: "preserved stdout".repeat(100),
                    cmd_stderr: "preserved stderr".repeat(100),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        for end in [
            action_end,
            End::Command(Default::default()),
            End::CommandCritical(Default::default()),
            End::ConnectToInstaller(Default::default()),
            End::InstallEventInfo(Default::default()),
            End::Load(Default::default()),
            End::LoadPackage(Default::default()),
            End::Analysis(Default::default()),
            End::Materialization(Default::default()),
            End::FileWatcher(Default::default()),
            End::ReUpload(Default::default()),
            End::ReSubmit(Default::default()),
            End::ExecutorStage(Default::default()),
            End::MatchDepFiles(buck2_data::MatchDepFilesEnd {
                outcome: 99,
                persisted_probe_us: Some(0),
                persisted_fetch_us: Some(u64::MAX),
                persisted_fetches: None,
            }),
            End::SpanCancelled(Default::default()),
        ] {
            let value = ended(end);
            assert_kept(cloned(&value), false);
            assert_kept(value, true);
        }
    }

    #[test]
    fn fallback_ends_keep_duration_presence_and_do_not_become_cancellation() {
        for duration in [
            None,
            Some(prost_types::Duration {
                seconds: 0,
                nanos: 0,
            }),
            Some(prost_types::Duration {
                seconds: -1,
                nanos: 1,
            }),
        ] {
            for phases in [false, true] {
                let mut value = ended(buck2_data::span_end_event::Data::Fake(Default::default()));
                if let StreamValue::Event(event) = &mut value {
                    let Some(buck2_data::buck_event::Data::SpanEnd(end)) = &mut event.data else {
                        unreachable!()
                    };
                    end.duration = duration;
                }
                let StreamValue::Event(event) = clean(value, phases) else {
                    unreachable!()
                };
                let Some(buck2_data::buck_event::Data::SpanEnd(end)) = event.data else {
                    panic!("end marker removed")
                };
                assert_eq!(end.duration, duration);
                assert!(end.data.is_none());
                assert!(end.stats.is_none());
            }
        }
    }

    #[test]
    fn instant_classifier_preserves_payloads_and_profile_timestamp_decisions() {
        use buck2_data::buck_event::Data as Event;
        use buck2_data::instant_event::Data as Instant;

        for (kind, payload) in [
            (
                InstantKind::ReSession,
                Instant::ReSession(Default::default()),
            ),
            (
                InstantKind::SystemInfo,
                Instant::SystemInfo(Default::default()),
            ),
            (InstantKind::Snapshot, Instant::Snapshot(Default::default())),
            (
                InstantKind::BuildGraphInfo,
                Instant::BuildGraphInfo(Default::default()),
            ),
            (
                InstantKind::TransitiveSetProjectionDef,
                Instant::TransitiveSetProjectionDef(Default::default()),
            ),
            (
                InstantKind::ActionGraphStart,
                Instant::ActionGraphStart(Default::default()),
            ),
            (
                InstantKind::ActionGraphFooter,
                Instant::ActionGraphFooter(Default::default()),
            ),
            (
                InstantKind::ConcurrentCommands,
                Instant::ConcurrentCommands(Default::default()),
            ),
            (
                InstantKind::InstallFinished,
                Instant::InstallFinished(Default::default()),
            ),
            (
                InstantKind::Ignored,
                Instant::UntrackedFile(Default::default()),
            ),
        ] {
            let value = instant(payload);
            for phases in [false, true] {
                for has_timestamp in [false, true] {
                    let StreamValue::Event(mut original) = cloned(&value) else {
                        unreachable!()
                    };
                    if !has_timestamp {
                        original.timestamp = None;
                    }
                    let Some(Event::Instant(before)) = &original.data else {
                        unreachable!()
                    };
                    assert_eq!(instant_kind(before.data.as_ref()), kind);
                    let mut raw = super::super::profile::Collector::new(Some(0), "run".to_owned());
                    raw.event(&original);
                    let StreamValue::Event(compacted) = clean(StreamValue::Event(original), phases)
                    else {
                        unreachable!()
                    };
                    let Some(Event::Instant(after)) = &compacted.data else {
                        panic!("instant marker removed")
                    };
                    assert_eq!(instant_kind(after.data.as_ref()), kind);
                    assert_eq!(after.data.is_some(), kind != InstantKind::Ignored);
                    let mut cleaned =
                        super::super::profile::Collector::new(Some(0), "run".to_owned());
                    cleaned.event(&compacted);
                    assert_eq!(raw.finish(false).unwrap(), cleaned.finish(false).unwrap());
                }
            }
        }
    }

    #[test]
    fn consumed_instants_are_not_validated_normalized_or_trimmed() {
        use buck2_data::instant_event::Data as Instant;
        for data in [
            Instant::ReSession(Default::default()),
            Instant::SystemInfo(Default::default()),
            Instant::Snapshot(Default::default()),
            Instant::BuildGraphInfo(Default::default()),
            Instant::InstallFinished(Default::default()),
            Instant::TransitiveSetProjectionDef(Default::default()),
            Instant::ActionGraphStart(buck2_data::ActionGraphStart { schema: 999 }),
            Instant::ActionGraphFooter(Default::default()),
        ] {
            let value = instant(data);
            assert_kept(cloned(&value), false);
            assert_kept(value, true);
        }
        let value = instant(Instant::UntrackedFile(buck2_data::UntrackedFile {
            path: "unused".to_owned(),
            file_type: "file".to_owned(),
        }));
        let StreamValue::Event(event) = clean(value, true) else {
            unreachable!()
        };
        let Some(buck2_data::buck_event::Data::Instant(instant)) = event.data else {
            unreachable!()
        };
        assert!(instant.data.is_none());
    }

    #[test]
    fn missing_timestamp_and_result_markers_preserve_scanner_record_semantics() {
        let mut value = started(buck2_data::span_start_event::Data::ActionExecution(
            Default::default(),
        ));
        if let StreamValue::Event(event) = &mut value {
            event.timestamp = None;
        }
        let StreamValue::Event(event) = clean(value, true) else {
            unreachable!()
        };
        assert!(event.timestamp.is_none() && event.data.is_some());
        assert_eq!((event.span_id, event.parent_id), (u64::MAX, 123));

        let result = StreamValue::Result(Box::new(buck2_cli_proto::CommandResult {
            result: Some(buck2_cli_proto::command_result::Result::PingResponse(
                Default::default(),
            )),
        }));
        let StreamValue::Result(result) = clean(result, false) else {
            panic!("Result marker removed")
        };
        assert!(matches!(
            result.result,
            Some(buck2_cli_proto::command_result::Result::PingResponse(_))
        ));
        let result = StreamValue::Result(Box::new(buck2_cli_proto::CommandResult {
            result: Some(buck2_cli_proto::command_result::Result::BuildResponse(
                Default::default(),
            )),
        }));
        assert_kept(result, false);
        for result in [
            buck2_cli_proto::command_result::Result::InstallResponse(Default::default()),
            buck2_cli_proto::command_result::Result::Error(Default::default()),
        ] {
            assert_kept(
                StreamValue::Result(Box::new(buck2_cli_proto::CommandResult {
                    result: Some(result),
                })),
                false,
            );
        }

        let partial = StreamValue::PartialResult(Box::new(buck2_cli_proto::PartialResult {
            partial_result: Some(buck2_cli_proto::partial_result::PartialResult::StdoutBytes(
                buck2_cli_proto::StdoutBytes {
                    data: vec![7; 4096],
                },
            )),
        }));
        let before = allocative::size_of_unique(&partial);
        let cleaned = clean(partial, false);
        assert!(allocative::size_of_unique(&cleaned) + 4096 <= before);
        let StreamValue::PartialResult(partial) = cleaned else {
            panic!("PartialResult marker removed")
        };
        assert!(partial.partial_result.is_none());
    }

    #[test]
    fn dropped_capacities_shrink_owned_charge_and_cleaning_is_idempotent() {
        let value = started(buck2_data::span_start_event::Data::Fake(
            buck2_data::FakeStart {
                caramba: "large unused event detail".repeat(1000),
            },
        ));
        let before = allocative::size_of_unique(&value);
        let cleaned = clean(value, false);
        assert!(before > allocative::size_of_unique(&cleaned) + 20_000);
        let expected = serde_json::to_value(&cleaned).unwrap();
        assert_eq!(
            serde_json::to_value(clean(cleaned, false)).unwrap(),
            expected
        );
        for start in [
            buck2_data::span_start_event::Data::ReUpload(Default::default()),
            buck2_data::span_start_event::Data::ReSubmit(Default::default()),
        ] {
            // The payload can be discarded, but the stage type cannot.
            assert_kept(started(start), false);
        }
    }

    #[test]
    fn timestamp_less_profile_kinds_keep_rejection_classification() {
        for value in [
            started(buck2_data::span_start_event::Data::Analysis(
                Default::default(),
            )),
            ended(buck2_data::span_end_event::Data::Analysis(
                Default::default(),
            )),
            instant(buck2_data::instant_event::Data::Snapshot(Default::default())),
            started(buck2_data::span_start_event::Data::ReUpload(
                Default::default(),
            )),
        ] {
            let StreamValue::Event(mut original) = value else {
                unreachable!()
            };
            original.timestamp = None;
            let mut raw = super::super::profile::Collector::new(Some(0), "run".to_owned());
            raw.event(&original);
            let StreamValue::Event(compacted) = clean(StreamValue::Event(original), false) else {
                unreachable!()
            };
            let mut cleaned = super::super::profile::Collector::new(Some(0), "run".to_owned());
            cleaned.event(&compacted);
            assert_eq!(raw.finish(false).unwrap(), cleaned.finish(false).unwrap());
        }
    }
}
