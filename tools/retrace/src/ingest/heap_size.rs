/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Exact allocation accounting for common protobuf shapes, not a wire-size
//! estimate. Most executor spans own only the outer BuckEvent box, its trace
//! ID, and a few stage strings. Visiting every inline protobuf field to count
//! these allocations is unnecessary.
//!
//! Counted structures deliberately use exhaustive patterns without `..`:
//! adding a protobuf field requires an accounting decision at compile time.
//! Scalar metadata is also checked for ownership and its exact inline type.
//! Untreated oneof variants and complicated messages use Allocative; the two
//! exhaustive stage matches instead require review when extended. Neither
//! silently gives a new variant a zero charge. String and vector
//! capacities, not lengths, are charged. This excludes the inline LogItem slot
//! exactly like size_of_unique_allocated_data; the batch owns that slot.

use std::mem::needs_drop;
use std::mem::size_of;

use buck2_event_log::stream_value::StreamValue;

// A new owned field in any supposedly scalar message must stop compilation.
// The typed bindings below additionally reject changing an inline Option to a
// Box, even though the message inside that box would still be heap-free.
const _: () = {
    assert!(!needs_drop::<prost_types::Timestamp>());
    assert!(!needs_drop::<prost_types::Duration>());
    assert!(!needs_drop::<buck2_data::SpanStats>());
};

#[cfg(test)]
pub(super) fn item_heap_bytes(item: &buck2_error::Result<StreamValue>) -> usize {
    match item {
        Ok(value) => value_heap_bytes(value),
        // Buck errors are Arc-backed; keep the existing reachable-allocation
        // accounting rather than treating a shared pointer as zero heap.
        Err(error) => {
            allocative::size_of_reachable(error).saturating_sub(size_of::<buck2_error::Error>())
        }
    }
}

pub(super) fn value_heap_bytes(value: &StreamValue) -> usize {
    match value {
        StreamValue::Event(event) => fast_event_heap(event)
            .unwrap_or_else(|| allocative::size_of_unique_allocated_data(value)),
        StreamValue::PartialResult(result) => fast_partial_heap(result)
            .unwrap_or_else(|| allocative::size_of_unique_allocated_data(value)),
        StreamValue::Result(_) => allocative::size_of_unique_allocated_data(value),
    }
}

fn optional_string(value: &Option<String>) -> usize {
    value.as_ref().map_or(0, String::capacity)
}

#[inline]
fn no_owned_heap<T>(_: &T) {
    const { assert!(!needs_drop::<T>()) };
}

fn fast_event_heap(event: &buck2_data::BuckEvent) -> Option<usize> {
    let buck2_data::BuckEvent {
        timestamp,
        trace_id,
        span_id,
        parent_id,
        data,
    } = event;
    let _: &Option<prost_types::Timestamp> = timestamp;
    no_owned_heap(span_id);
    no_owned_heap(parent_id);
    let payload = match data {
        None => 0,
        Some(buck2_data::buck_event::Data::SpanStart(
            start @ buck2_data::SpanStartEvent { data: _ },
        )) => start_heap(start)?,
        Some(buck2_data::buck_event::Data::SpanEnd(
            end @ buck2_data::SpanEndEvent {
                stats: _,
                duration: _,
                data: _,
            },
        )) => end_heap(end)?,
        _ => return None,
    };
    Some(size_of::<buck2_data::BuckEvent>() + trace_id.capacity() + payload)
}

fn start_heap(start: &buck2_data::SpanStartEvent) -> Option<usize> {
    use buck2_data::span_start_event::Data;
    let buck2_data::SpanStartEvent { data } = start;
    Some(match data {
        None => 0,
        Some(Data::ExecutorStage(stage @ buck2_data::ExecutorStageStart { stage: _ })) => {
            executor_heap(stage)?
        }
        Some(Data::SharedTask(buck2_data::SharedTaskStart { owner_trace_id })) => {
            owner_trace_id.capacity()
        }
        Some(Data::Materialization(buck2_data::MaterializationStart { action_digest })) => {
            optional_string(action_digest)
        }
        Some(Data::MatchDepFiles(buck2_data::MatchDepFilesStart {
            checking_filtered_inputs,
            remote_cache,
        })) => {
            no_owned_heap(checking_filtered_inputs);
            no_owned_heap(remote_cache);
            0
        }
        Some(Data::ReUpload(buck2_data::ReUploadStart {})) => 0,
        Some(Data::ReSubmit(buck2_data::ReSubmitStart {})) => 0,
        Some(Data::Fake(buck2_data::FakeStart { caramba })) => caramba.capacity(),
        _ => return None,
    })
}

fn end_heap(end: &buck2_data::SpanEndEvent) -> Option<usize> {
    use buck2_data::span_end_event::Data;
    let buck2_data::SpanEndEvent {
        stats,
        duration,
        data,
    } = end;
    let _: &Option<buck2_data::SpanStats> = stats;
    let _: &Option<prost_types::Duration> = duration;
    Some(match data {
        None => 0,
        Some(Data::ExecutorStage(buck2_data::ExecutorStageEnd {
            cache_query_error: None,
            cache_query,
            cache_hit,
        })) => {
            let _: &Option<buck2_data::CacheQueryTimings> = cache_query;
            let _: &Option<buck2_data::CacheHitTimings> = cache_hit;
            const {
                assert!(!needs_drop::<buck2_data::CacheQueryTimings>());
                assert!(!needs_drop::<buck2_data::CacheHitTimings>());
            };
            0
        }
        Some(Data::MatchDepFiles(buck2_data::MatchDepFilesEnd {
            outcome,
            persisted_probe_us,
            persisted_fetch_us,
            persisted_fetches,
        })) => {
            no_owned_heap(outcome);
            let _: &Option<u64> = persisted_probe_us;
            let _: &Option<u64> = persisted_fetch_us;
            let _: &Option<u64> = persisted_fetches;
            0
        }
        Some(Data::SpanCancelled(buck2_data::SpanCancelled {})) => 0,
        Some(Data::AnalysisResolveQueries(buck2_data::AnalysisResolveQueriesEnd {})) => 0,
        Some(Data::AnalysisStage(buck2_data::AnalysisStageEnd {})) => 0,
        Some(Data::SharedTask(buck2_data::SharedTaskEnd {})) => 0,
        Some(Data::DeferredPreparationStage(buck2_data::DeferredPreparationStageEnd {})) => 0,
        Some(Data::DeferredEvaluation(buck2_data::DynamicLambdaEnd {})) => 0,
        Some(Data::Fake(buck2_data::FakeEnd {})) => 0,
        Some(Data::ReSubmit(buck2_data::ReSubmitEnd {
            slot_wait_us: _,
            request_prep_us: _,
            rpc_open_us: _,
            free_slots_hint: _,
        })) => {
            const { assert!(!needs_drop::<buck2_data::ReSubmitEnd>()) };
            0
        }
        Some(Data::Materialization(buck2_data::MaterializationEnd {
            file_count,
            total_bytes,
            path,
            action_digest,
            success,
            error,
            method,
            url,
        })) => {
            no_owned_heap(file_count);
            no_owned_heap(total_bytes);
            no_owned_heap(success);
            no_owned_heap(method);
            path.capacity()
                + optional_string(action_digest)
                + optional_string(error)
                + optional_string(url)
        }
        _ => return None,
    })
}

fn executor_heap(executor: &buck2_data::ExecutorStageStart) -> Option<usize> {
    use buck2_data::executor_stage_start::Stage;
    let buck2_data::ExecutorStageStart { stage } = executor;
    Some(match stage {
        None => 0,
        Some(Stage::Re(re @ buck2_data::ReStage { stage: _ })) => remote_heap(re)?,
        Some(Stage::Local(local @ buck2_data::LocalStage { stage: _ })) => local_heap(local)?,
        Some(Stage::CacheQuery(buck2_data::CacheQuery {
            action_digest,
            cache_type,
        })) => {
            no_owned_heap(cache_type);
            action_digest.capacity()
        }
        Some(Stage::CacheHit(buck2_data::CacheHit {
            action_digest,
            action_key,
            cache_type,
        })) => {
            no_owned_heap(cache_type);
            action_digest.capacity() + optional_string(action_key)
        }
        Some(Stage::Prepare(buck2_data::PrepareAction {})) => 0,
        Some(Stage::CasArtifact(buck2_data::CasArtifactStage { phase })) => {
            no_owned_heap(phase);
            0
        }
    })
}

fn queue_heap(queue: &buck2_data::ReQueue) -> usize {
    let buck2_data::ReQueue {
        action_digest,
        use_case,
    } = queue;
    action_digest.capacity() + use_case.capacity()
}

fn platform_heap(platform: &buck2_data::RePlatform) -> usize {
    let buck2_data::RePlatform { properties } = platform;
    properties.capacity() * size_of::<buck2_data::re_platform::Property>()
        + properties
            .iter()
            .map(|buck2_data::re_platform::Property { name, value }| {
                name.capacity() + value.capacity()
            })
            .sum::<usize>()
}

fn remote_heap(re: &buck2_data::ReStage) -> Option<usize> {
    use buck2_data::re_stage::Stage;
    let buck2_data::ReStage { stage } = re;
    Some(match stage {
        None => 0,
        Some(Stage::Execute(buck2_data::ReExecute {
            action_digest,
            platform,
            action_key,
            use_case,
            persistent_worker,
        })) => {
            no_owned_heap(persistent_worker);
            action_digest.capacity()
                + platform.as_ref().map_or(0, platform_heap)
                + optional_string(action_key)
                + use_case.capacity()
        }
        Some(Stage::Download(buck2_data::ReDownload {})) => 0,
        Some(Stage::Queue(
            queue @ buck2_data::ReQueue {
                action_digest: _,
                use_case: _,
            },
        )) => queue_heap(queue),
        Some(Stage::WorkerDownload(buck2_data::ReWorkerDownload {
            action_digest,
            use_case,
        })) => action_digest.capacity() + use_case.capacity(),
        Some(Stage::WorkerUpload(buck2_data::ReWorkerUpload {
            action_digest,
            use_case,
        })) => action_digest.capacity() + use_case.capacity(),
        Some(Stage::Unknown(buck2_data::ReUnknown { action_digest })) => action_digest.capacity(),
        Some(Stage::MaterializeFailedInputs(buck2_data::MaterializeFailedInputs {})) => 0,
        Some(Stage::BeforeActionExecution(buck2_data::ReBeforeAction { action_digest })) => {
            action_digest.capacity()
        }
        Some(Stage::AfterActionExecution(buck2_data::ReAfterAction { action_digest })) => {
            action_digest.capacity()
        }
        Some(Stage::QueueOverQuota(buck2_data::ReQueueOverQuota {
            queue_info,
            exceeded_quotas,
        })) => {
            queue_info.as_ref().map_or(0, queue_heap)
                + exceeded_quotas.capacity() * size_of::<String>()
                + exceeded_quotas.iter().map(String::capacity).sum::<usize>()
        }
        Some(Stage::QueueNoWorkerAvailable(buck2_data::ReQueueNoWorkerAvailable {
            queue_info,
        }))
        | Some(Stage::QueueAcquiringDependencies(buck2_data::ReQueueAcquiringDependencies {
            queue_info,
        }))
        | Some(Stage::QueueCancelled(buck2_data::ReQueueCancelled { queue_info })) => {
            queue_info.as_ref().map_or(0, queue_heap)
        }
    })
}

fn local_heap(local: &buck2_data::LocalStage) -> Option<usize> {
    use buck2_data::local_stage::Stage;
    let buck2_data::LocalStage { stage } = local;
    Some(match stage {
        None => 0,
        Some(Stage::Queued(buck2_data::LocalQueued {})) => 0,
        Some(Stage::MaterializeInputs(buck2_data::LocalMaterializeInputs {})) => 0,
        Some(Stage::PrepareOutputs(buck2_data::LocalPrepareOutputDirs {})) => 0,
        Some(Stage::AcquireLocalResource(buck2_data::AcquireLocalResource {})) => 0,
        Some(Stage::WorkerQueued(buck2_data::WorkerQueued {})) => 0,
        Some(Stage::WorkerWait(buck2_data::WorkerWait {})) => 0,
        // Command argv/environment/resource payloads keep the reference walk.
        _ => return None,
    })
}

fn fast_partial_heap(result: &buck2_cli_proto::PartialResult) -> Option<usize> {
    use buck2_cli_proto::partial_result::PartialResult;
    let buck2_cli_proto::PartialResult { partial_result } = result;
    let payload = match partial_result {
        None => 0,
        Some(PartialResult::StdoutBytes(buck2_cli_proto::StdoutBytes { data })) => data.capacity(),
        Some(PartialResult::LspMessage(buck2_cli_proto::LspMessage { lsp_json })) => {
            lsp_json.capacity()
        }
        Some(PartialResult::DapMessage(buck2_cli_proto::DapMessage { dap_json })) => {
            dap_json.capacity()
        }
        _ => return None,
    };
    Some(size_of::<buck2_cli_proto::PartialResult>() + payload)
}

#[cfg(test)]
mod tests {
    use prost::Message as _;

    use super::*;

    fn spare(value: &str) -> String {
        let mut string = String::with_capacity(value.len() + 97);
        string.push_str(value);
        string
    }

    fn event(data: Option<buck2_data::buck_event::Data>) -> buck2_data::BuckEvent {
        buck2_data::BuckEvent {
            timestamp: Some(prost_types::Timestamp {
                seconds: 123,
                nanos: 456,
            }),
            trace_id: spare("capture"),
            span_id: 11,
            parent_id: 7,
            data,
        }
    }

    fn assert_event(event: buck2_data::BuckEvent, fast: bool) {
        assert_eq!(fast_event_heap(&event).is_some(), fast);
        let value = StreamValue::Event(Box::new(event));
        let expected = allocative::size_of_unique_allocated_data(&value);
        assert_eq!(item_heap_bytes(&Ok(value)), expected);
    }

    fn start(data: buck2_data::span_start_event::Data) -> buck2_data::BuckEvent {
        event(Some(buck2_data::buck_event::Data::SpanStart(
            buck2_data::SpanStartEvent { data: Some(data) },
        )))
    }

    fn executor(stage: Option<buck2_data::executor_stage_start::Stage>) -> buck2_data::BuckEvent {
        start(buck2_data::span_start_event::Data::ExecutorStage(
            buck2_data::ExecutorStageStart { stage },
        ))
    }

    #[test]
    fn local_cache_scalar_payloads_match_reference_allocation_accounting() {
        for remote_cache in [false, true] {
            assert_event(
                start(buck2_data::span_start_event::Data::MatchDepFiles(
                    buck2_data::MatchDepFilesStart {
                        checking_filtered_inputs: true,
                        remote_cache,
                    },
                )),
                true,
            );
        }
        for persisted_probe_us in [None, Some(0), Some(u64::MAX)] {
            assert_event(
                event(Some(buck2_data::buck_event::Data::SpanEnd(
                    buck2_data::SpanEndEvent {
                        data: Some(buck2_data::span_end_event::Data::MatchDepFiles(
                            buck2_data::MatchDepFilesEnd {
                                outcome: 99,
                                persisted_probe_us,
                                persisted_fetch_us: Some(9),
                                persisted_fetches: Some(3),
                            },
                        )),
                        ..Default::default()
                    },
                ))),
                true,
            );
        }
    }

    fn queue() -> buck2_data::ReQueue {
        buck2_data::ReQueue {
            action_digest: spare("digest"),
            use_case: spare(""),
        }
    }

    #[test]
    fn outer_box_trace_capacity_and_absent_payloads_match_allocative() {
        assert_event(event(None), true);
        assert_event(buck2_data::BuckEvent::default(), true);
        assert_event(
            event(Some(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent { data: None },
            ))),
            true,
        );
        assert_event(executor(None), true);
    }

    #[test]
    fn every_counted_end_preserves_inline_metadata_and_string_capacity() {
        use buck2_data::span_end_event::Data;
        let variants = vec![
            None,
            Some(Data::ExecutorStage(Default::default())),
            Some(Data::ExecutorStage(buck2_data::ExecutorStageEnd {
                cache_query: Some(buck2_data::CacheQueryTimings {
                    slot_wait_us: 0,
                    remote_lookup_us: 45,
                }),
                ..Default::default()
            })),
            Some(Data::ExecutorStage(buck2_data::ExecutorStageEnd {
                cache_hit: Some(buck2_data::CacheHitTimings {
                    tree_admission_us: Some(0),
                    tree_fetch_us: Some(12),
                    local_bookkeeping_us: Some(3),
                    declare_us: Some(15),
                    tree_count: 2,
                }),
                ..Default::default()
            })),
            Some(Data::ExecutorStage(buck2_data::ExecutorStageEnd {
                cache_query: Some(Default::default()),
                cache_hit: Some(Default::default()),
                ..Default::default()
            })),
            Some(Data::SpanCancelled(buck2_data::SpanCancelled {})),
            Some(Data::AnalysisResolveQueries(
                buck2_data::AnalysisResolveQueriesEnd {},
            )),
            Some(Data::AnalysisStage(buck2_data::AnalysisStageEnd {})),
            Some(Data::SharedTask(buck2_data::SharedTaskEnd {})),
            Some(Data::DeferredPreparationStage(
                buck2_data::DeferredPreparationStageEnd {},
            )),
            Some(Data::DeferredEvaluation(buck2_data::DynamicLambdaEnd {})),
            Some(Data::Fake(buck2_data::FakeEnd {})),
            Some(Data::ReSubmit(buck2_data::ReSubmitEnd {
                slot_wait_us: 1,
                request_prep_us: 2,
                rpc_open_us: 3,
                free_slots_hint: 0,
            })),
            Some(Data::Materialization(buck2_data::MaterializationEnd {
                file_count: 1,
                total_bytes: 2,
                path: spare("path"),
                action_digest: Some(spare("")),
                success: false,
                error: Some(spare("failure")),
                method: Some(0),
                url: None,
            })),
        ];
        for data in variants {
            assert_event(
                event(Some(buck2_data::buck_event::Data::SpanEnd(
                    buck2_data::SpanEndEvent {
                        stats: Some(buck2_data::SpanStats {
                            max_poll_time_us: 3,
                            total_poll_time_us: 4,
                        }),
                        duration: Some(prost_types::Duration {
                            seconds: 9,
                            nanos: 12,
                        }),
                        data,
                    },
                ))),
                true,
            );
        }
        // Only an absent cache-query error is heap-free. Present payloads
        // retain the reference walker, including empty strings whose spare
        // capacity is still owned and must be charged to the decoded batch.
        for error in [
            buck2_data::CacheQueryError {
                error: spare("lookup failed"),
                re_error_code: None,
            },
            buck2_data::CacheQueryError {
                error: spare(""),
                re_error_code: Some(spare("")),
            },
            buck2_data::CacheQueryError {
                error: spare("lookup failed"),
                re_error_code: Some(spare("UNAVAILABLE")),
            },
        ] {
            assert_event(
                event(Some(buck2_data::buck_event::Data::SpanEnd(
                    buck2_data::SpanEndEvent {
                        data: Some(Data::ExecutorStage(buck2_data::ExecutorStageEnd {
                            cache_query_error: Some(error),
                            ..Default::default()
                        })),
                        ..Default::default()
                    },
                ))),
                false,
            );
        }
        assert_event(
            event(Some(buck2_data::buck_event::Data::SpanEnd(
                Default::default(),
            ))),
            true,
        );
    }

    #[test]
    fn every_counted_start_and_local_stage_matches_allocative() {
        use buck2_data::span_start_event::Data;
        for data in [
            Data::SharedTask(buck2_data::SharedTaskStart {
                owner_trace_id: spare("other"),
            }),
            Data::Materialization(buck2_data::MaterializationStart {
                action_digest: Some(spare("")),
            }),
            Data::Materialization(buck2_data::MaterializationStart {
                action_digest: None,
            }),
            Data::ReUpload(buck2_data::ReUploadStart {}),
            Data::ReSubmit(buck2_data::ReSubmitStart {}),
            Data::Fake(buck2_data::FakeStart {
                caramba: spare("x"),
            }),
        ] {
            assert_event(start(data), true);
        }
        use buck2_data::executor_stage_start::Stage as Executor;
        for stage in [
            Executor::CacheQuery(buck2_data::CacheQuery {
                action_digest: spare("query"),
                cache_type: 1,
            }),
            Executor::CacheHit(buck2_data::CacheHit {
                action_digest: spare("hit"),
                action_key: Some(spare("")),
                cache_type: 0,
            }),
            Executor::CacheHit(buck2_data::CacheHit {
                action_digest: spare("hit"),
                action_key: None,
                cache_type: 1,
            }),
            Executor::Prepare(buck2_data::PrepareAction {}),
        ] {
            assert_event(executor(Some(stage)), true);
        }
        use buck2_data::local_stage::Stage;
        for stage in [
            None,
            Some(Stage::Queued(buck2_data::LocalQueued {})),
            Some(Stage::MaterializeInputs(
                buck2_data::LocalMaterializeInputs {},
            )),
            Some(Stage::PrepareOutputs(buck2_data::LocalPrepareOutputDirs {})),
            Some(Stage::AcquireLocalResource(
                buck2_data::AcquireLocalResource {},
            )),
            Some(Stage::WorkerQueued(buck2_data::WorkerQueued {})),
            Some(Stage::WorkerWait(buck2_data::WorkerWait {})),
        ] {
            assert_event(
                executor(Some(Executor::Local(buck2_data::LocalStage { stage }))),
                true,
            );
        }
    }

    #[test]
    fn every_remote_stage_and_nested_queue_matches_allocative() {
        use buck2_data::re_stage::Stage;
        let mut properties = Vec::with_capacity(9);
        properties.push(buck2_data::re_platform::Property {
            name: spare("platform"),
            value: spare("linux"),
        });
        properties.push(buck2_data::re_platform::Property {
            name: spare(""),
            value: spare(""),
        });
        let stages = vec![
            None,
            Some(Stage::Execute(buck2_data::ReExecute {
                action_digest: spare("execute"),
                platform: Some(buck2_data::RePlatform { properties }),
                action_key: Some(spare("key")),
                use_case: spare("buck"),
                persistent_worker: true,
            })),
            Some(Stage::Execute(Default::default())),
            Some(Stage::Download(buck2_data::ReDownload {})),
            Some(Stage::Queue(queue())),
            Some(Stage::WorkerDownload(buck2_data::ReWorkerDownload {
                action_digest: spare("digest"),
                use_case: spare("case"),
            })),
            Some(Stage::WorkerUpload(buck2_data::ReWorkerUpload {
                action_digest: spare("digest"),
                use_case: spare("case"),
            })),
            Some(Stage::Unknown(buck2_data::ReUnknown {
                action_digest: spare("x"),
            })),
            Some(Stage::MaterializeFailedInputs(
                buck2_data::MaterializeFailedInputs {},
            )),
            Some(Stage::BeforeActionExecution(buck2_data::ReBeforeAction {
                action_digest: spare("before"),
            })),
            Some(Stage::AfterActionExecution(buck2_data::ReAfterAction {
                action_digest: spare("after"),
            })),
            Some(Stage::QueueOverQuota(buck2_data::ReQueueOverQuota {
                queue_info: Some(queue()),
                exceeded_quotas: vec![spare("worker_units:pool:platform")],
            })),
            Some(Stage::QueueNoWorkerAvailable(
                buck2_data::ReQueueNoWorkerAvailable {
                    queue_info: Some(queue()),
                },
            )),
            Some(Stage::QueueAcquiringDependencies(
                buck2_data::ReQueueAcquiringDependencies {
                    queue_info: Some(queue()),
                },
            )),
            Some(Stage::QueueCancelled(buck2_data::ReQueueCancelled {
                queue_info: Some(queue()),
            })),
            Some(Stage::QueueOverQuota(Default::default())),
            Some(Stage::QueueNoWorkerAvailable(Default::default())),
            Some(Stage::QueueAcquiringDependencies(Default::default())),
            Some(Stage::QueueCancelled(Default::default())),
        ];
        for stage in stages {
            assert_event(
                executor(Some(buck2_data::executor_stage_start::Stage::Re(
                    buck2_data::ReStage { stage },
                ))),
                true,
            );
        }
        // Spare vector capacity counts even when no properties are populated.
        assert_event(
            executor(Some(buck2_data::executor_stage_start::Stage::Re(
                buck2_data::ReStage {
                    stage: Some(Stage::Execute(buck2_data::ReExecute {
                        platform: Some(buck2_data::RePlatform {
                            properties: Vec::with_capacity(23),
                        }),
                        ..Default::default()
                    })),
                },
            ))),
            true,
        );
    }

    #[test]
    fn partial_result_spare_bytes_and_strings_match_allocative() {
        use buck2_cli_proto::partial_result::PartialResult;
        let mut bytes = Vec::with_capacity(4096);
        bytes.push(7);
        for partial_result in [
            None,
            Some(PartialResult::StdoutBytes(buck2_cli_proto::StdoutBytes {
                data: bytes,
            })),
            Some(PartialResult::StdoutBytes(buck2_cli_proto::StdoutBytes {
                data: Vec::with_capacity(513),
            })),
            Some(PartialResult::LspMessage(buck2_cli_proto::LspMessage {
                lsp_json: spare("{}"),
            })),
            Some(PartialResult::DapMessage(buck2_cli_proto::DapMessage {
                dap_json: Vec::with_capacity(129),
            })),
        ] {
            let result = buck2_cli_proto::PartialResult { partial_result };
            assert!(fast_partial_heap(&result).is_some());
            let value = StreamValue::PartialResult(Box::new(result));
            let expected = allocative::size_of_unique_allocated_data(&value);
            assert_eq!(item_heap_bytes(&Ok(value)), expected);
        }
    }

    #[test]
    fn complex_records_use_the_reference_walker_instead_of_a_partial_charge() {
        let mut argv = Vec::with_capacity(31);
        argv.push(spare("compiler"));
        assert_event(
            executor(Some(buck2_data::executor_stage_start::Stage::Local(
                buck2_data::LocalStage {
                    stage: Some(buck2_data::local_stage::Stage::Execute(
                        buck2_data::LocalExecute {
                            command: Some(buck2_data::LocalCommand {
                                argv,
                                ..Default::default()
                            }),
                        },
                    )),
                },
            ))),
            false,
        );
        let mut reused_ids = Vec::with_capacity(71);
        reused_ids.extend_from_slice(&[1, 2, 3]);
        assert_event(
            event(Some(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                        buck2_data::ActionExecutionEnd {
                            graph_producers: Some(buck2_data::ActionGraphProducers {
                                reused_ids,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            ))),
            false,
        );
        for data in [
            buck2_data::buck_event::Data::Instant(Default::default()),
            buck2_data::buck_event::Data::Record(Default::default()),
        ] {
            assert_event(event(Some(data)), false);
        }
        let value = StreamValue::Result(Box::default());
        let expected = allocative::size_of_unique_allocated_data(&value);
        assert_eq!(item_heap_bytes(&Ok(value)), expected);
        let value = StreamValue::PartialResult(Box::new(buck2_cli_proto::PartialResult {
            partial_result: Some(
                buck2_cli_proto::partial_result::PartialResult::SubscriptionResponseWrapper(
                    Default::default(),
                ),
            ),
        }));
        let expected = allocative::size_of_unique_allocated_data(&value);
        assert_eq!(item_heap_bytes(&Ok(value)), expected);
        let error = buck2_error::buck2_error!(buck2_error::ErrorTag::Input, "fixture error")
            .context("outer context");
        let expected =
            allocative::size_of_reachable(&error).saturating_sub(size_of::<buck2_error::Error>());
        assert_eq!(item_heap_bytes(&Err(error)), expected);
    }

    #[test]
    fn decoded_messages_and_unknown_wire_fields_keep_exact_accounting() {
        let mut encoded = executor(Some(buck2_data::executor_stage_start::Stage::Re(
            buck2_data::ReStage {
                stage: Some(buck2_data::re_stage::Stage::Queue(queue())),
            },
        )))
        .encode_to_vec();
        // Unknown length-delimited field 2046. Prost discards unknown fields;
        // do not charge their wire bytes as retained decoded allocations.
        encoded.extend_from_slice(&[0xf2, 0x7f, 3, 1, 2, 3]);
        let decoded = buck2_data::BuckEvent::decode(encoded.as_slice()).unwrap();
        assert_event(decoded, true);
    }
}
