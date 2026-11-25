/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Synthetic event-log builder for tests: encodes a small but
//! representative build (a local action, a hybrid local-vs-RE race
//! with a cancelled loser, ReUpload/ReSubmit-era stages, an
//! integer-id tset projection, the schema footer and the client
//! result) as real length-delimited protobuf frames.  Used by the
//! in-crate ingest tests and the CLI acceptance suite; not part of
//! the tool's API.
#![doc(hidden)]

use std::path::Path;
use std::path::PathBuf;

use prost::Message as _;

pub const RUN_ID: &str = "11111111-2222-3333-4444-555555555555";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandOnlyResult {
    Success,
    Error,
    TestFailure,
    Missing,
    Unrecognized,
    Truncated,
}

/// Real protobuf frames with command evidence but no action rows or graph.
pub fn command_only_log(dir: &Path, result: CommandOnlyResult, phases: bool) -> PathBuf {
    use buck2_cli_proto::command_result::Result as R;
    use buck2_data::span_end_event::Data as E;
    use buck2_data::span_start_event::Data as S;
    let (name, kind) = match result {
        CommandOnlyResult::Error => (
            "uquery",
            buck2_data::command_start::Data::Query(Default::default()),
        ),
        CommandOnlyResult::TestFailure => (
            "test",
            buck2_data::command_start::Data::Test(Default::default()),
        ),
        _ => (
            "aquery",
            buck2_data::command_start::Data::Aquery(Default::default()),
        ),
    };
    let mut frames = vec![
        buck2_data::Invocation {
            command_line_args: vec![
                "buck2".to_owned(),
                name.to_owned(),
                "fbcode//demo:t".to_owned(),
            ],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
    ];
    frames.push(event(
        1000,
        1,
        0,
        span_start(S::Command(buck2_data::CommandStart {
            data: Some(kind),
            ..Default::default()
        })),
    ));
    if phases {
        frames.push(event(
            2000,
            2,
            1,
            span_start(S::Analysis(Default::default())),
        ));
        frames.push(event(
            42000,
            2,
            1,
            span_end(40000, E::Analysis(Default::default())),
        ));
        frames.push(event(
            45000,
            0,
            0,
            instant(buck2_data::instant_event::Data::BuildGraphInfo(
                buck2_data::BuildGraphExecutionInfo {
                    critical_path2: vec![buck2_data::CriticalPathEntry2 {
                        entry: Some(buck2_data::critical_path_entry2::Entry::Analysis(
                            Default::default(),
                        )),
                        duration: Some(dur(40000)),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )),
        ));
    }
    if result != CommandOnlyResult::Truncated {
        frames.push(event(
            51000,
            1,
            0,
            span_end(50000, E::Command(Default::default())),
        ));
    }
    let response = match result {
        CommandOnlyResult::Success => Some(R::AqueryResponse(Default::default())),
        CommandOnlyResult::Error => Some(R::Error(buck2_data::ErrorReport {
            message: "Query args supplied without any `%s` placeholder in the query".to_owned(),
            ..Default::default()
        })),
        CommandOnlyResult::TestFailure => Some(R::TestResponse(buck2_cli_proto::TestResponse {
            executor_exit_code: 42,
            ..Default::default()
        })),
        CommandOnlyResult::Unrecognized => Some(R::NewGenericResponseMessage(Default::default())),
        CommandOnlyResult::Missing | CommandOnlyResult::Truncated => None,
    };
    if let Some(response) = response {
        frames.push(frame(buck2_cli_proto::command_progress::Progress::Result(
            Box::new(buck2_cli_proto::CommandResult {
                result: Some(response),
                ..Default::default()
            }),
        )));
    }
    let path = dir.join(format!("command-only-{result:?}-{phases}.pb"));
    std::fs::write(&path, frames.concat()).unwrap();
    path
}

const T0: i64 = 1_700_000_000; // seconds

fn ts(off_us: i64) -> ::prost_types::Timestamp {
    ::prost_types::Timestamp {
        seconds: T0 + off_us / 1_000_000,
        nanos: i32::try_from((off_us % 1_000_000) * 1_000).unwrap(),
    }
}

fn dur(us: i64) -> ::prost_types::Duration {
    ::prost_types::Duration {
        seconds: us / 1_000_000,
        nanos: i32::try_from((us % 1_000_000) * 1_000).unwrap(),
    }
}

fn frame(progress: buck2_cli_proto::command_progress::Progress) -> Vec<u8> {
    buck2_cli_proto::CommandProgress {
        progress: Some(progress),
    }
    .encode_length_delimited_to_vec()
}

fn event(off_us: i64, span_id: u64, parent_id: u64, data: buck2_data::buck_event::Data) -> Vec<u8> {
    frame(buck2_cli_proto::command_progress::Progress::Event(
        Box::new(buck2_data::BuckEvent {
            timestamp: Some(ts(off_us)),
            trace_id: RUN_ID.to_owned(),
            span_id,
            parent_id,
            data: Some(data),
        }),
    ))
}

fn span_start(data: buck2_data::span_start_event::Data) -> buck2_data::buck_event::Data {
    buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent { data: Some(data) })
}

fn span_end(dur_us_: i64, data: buck2_data::span_end_event::Data) -> buck2_data::buck_event::Data {
    buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
        duration: Some(dur(dur_us_)),
        data: Some(data),
        ..Default::default()
    })
}

fn instant(data: buck2_data::instant_event::Data) -> buck2_data::buck_event::Data {
    buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent { data: Some(data) })
}

fn action_key(n: u8) -> buck2_data::ActionKey {
    buck2_data::ActionKey {
        id: vec![n],
        owner: Some(buck2_data::action_key::Owner::TargetLabel(
            buck2_data::ConfiguredTargetLabel {
                label: Some(buck2_data::TargetLabel {
                    package: "fbcode//demo".to_owned(),
                    name: format!("t{n}"),
                }),
                configuration: Some(buck2_data::Configuration {
                    full_name: "cfg:demo".to_owned(),
                }),
                ..Default::default()
            },
        )),
        key: format!("k{n}"),
    }
}

fn action_start(n: u8, category: &str) -> buck2_data::span_start_event::Data {
    buck2_data::span_start_event::Data::ActionExecution(buck2_data::ActionExecutionStart {
        key: Some(action_key(n)),
        kind: 3, // Run
        name: Some(buck2_data::ActionName {
            category: category.to_owned(),
            identifier: String::new(),
        }),
    })
}

fn local_success(argv: &[&str], exec_us: i64) -> buck2_data::CommandExecution {
    buck2_data::CommandExecution {
        details: Some(buck2_data::CommandExecutionDetails {
            signed_exit_code: Some(0),
            command_kind: Some(buck2_data::CommandExecutionKind {
                command: Some(buck2_data::command_execution_kind::Command::LocalCommand(
                    buck2_data::LocalCommand {
                        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
                        ..Default::default()
                    },
                )),
            }),
            metadata: Some(buck2_data::CommandExecutionMetadata {
                wall_time: Some(dur(exec_us)),
                execution_time: Some(dur(exec_us)),
                ..Default::default()
            }),
            ..Default::default()
        }),
        status: Some(buck2_data::command_execution::Status::Success(
            buck2_data::command_execution::Success {},
        )),
        ..Default::default()
    }
}

/// Two actions: `t1` runs locally; `t2` wins on RE after a
/// cancelled local attempt, depends on `t1` by span id and on a
/// tset projection by integer id.  Complete log.
pub fn demo_log(
    dir: &Path,
    footer_schema: Option<u32>,
    define_tset: bool,
    with_result: bool,
) -> PathBuf {
    demo_log_opts(dir, footer_schema, define_tset, with_result, true)
}

/// [`demo_log`] with the start stamp controllable: a stampless log
/// with a foreign FOOTER exercises the end-of-scan refusal on its own
/// (old recorders wrote footers before the start stamp existed).
pub fn demo_log_opts(
    dir: &Path,
    footer_schema: Option<u32>,
    define_tset: bool,
    with_result: bool,
    with_start_stamp: bool,
) -> PathBuf {
    demo_log_with(
        dir,
        DemoOpts {
            footer: footer_schema.map(|schema| FooterSpec::exact(schema, define_tset)),
            define_tset,
            with_result,
            with_start_stamp,
            with_command_end: true,
        },
    )
}

/// The action-graph footer a demo log declares.
#[derive(Clone, Copy, Debug)]
pub struct FooterSpec {
    pub schema: u32,
    pub action_ends: u64,
    pub tset_def_events: u64,
    pub structure_defs: u64,
    pub aliases: u64,
    /// Declare the graph incomplete, as for a failed build.
    pub nonfinal: bool,
    /// Declare that the definitions walk could not finish.
    pub unfinished_walk: bool,
}

impl FooterSpec {
    /// A footer that vouches for what [`demo_log`] records: two action ends, and one
    /// definition when `define_tset`.
    pub fn exact(schema: u32, define_tset: bool) -> FooterSpec {
        FooterSpec {
            schema,
            action_ends: 2,
            tset_def_events: u64::from(define_tset),
            structure_defs: 0,
            aliases: 0,
            nonfinal: false,
            unfinished_walk: false,
        }
    }
}

/// Everything [`demo_log`] lets a test vary.
#[derive(Clone, Copy, Debug)]
pub struct DemoOpts {
    pub footer: Option<FooterSpec>,
    pub define_tset: bool,
    pub with_result: bool,
    pub with_start_stamp: bool,
    pub with_command_end: bool,
}

pub fn demo_log_with(dir: &Path, o: DemoOpts) -> PathBuf {
    let DemoOpts {
        footer,
        define_tset,
        with_result,
        with_start_stamp,
        with_command_end,
    } = o;
    let mut frames: Vec<Vec<u8>> = vec![
        buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned(), "build".to_owned()],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
    ];
    frames.push(event(
        0,
        1,
        0,
        span_start(buck2_data::span_start_event::Data::Command(
            buck2_data::CommandStart {
                data: Some(buck2_data::command_start::Data::Build(Default::default())),
                ..Default::default()
            },
        )),
    ));
    // The start stamp mirrors the footer's declaration, the way the
    // recorder emits it at command start.
    if let Some(spec) = footer.filter(|_| with_start_stamp) {
        frames.push(event(
            500,
            0,
            0,
            instant(buck2_data::instant_event::Data::ActionGraphStart(
                buck2_data::ActionGraphStart {
                    schema: spec.schema,
                },
            )),
        ));
    }
    // t1: local action, 100ms.
    frames.push(event(1_000, 10, 1, span_start(action_start(1, "rustc"))));
    frames.push(event(
        101_000,
        10,
        1,
        span_end(
            100_000,
            buck2_data::span_end_event::Data::ActionExecution(Box::new(
                buck2_data::ActionExecutionEnd {
                    key: Some(action_key(1)),
                    kind: 3,
                    name: Some(buck2_data::ActionName {
                        category: "rustc".to_owned(),
                        identifier: String::new(),
                    }),
                    execution_kind: 1, // local
                    commands: vec![local_success(&["rustc", "lib.rs"], 90_000)],
                    output_size: 4096,
                    ..Default::default()
                },
            )),
        ),
    ));
    // The first projection has ID zero, matching the producer's dense allocator.
    if define_tset {
        frames.push(event(
            2_000,
            0,
            0,
            instant(buck2_data::instant_event::Data::TransitiveSetProjectionDef(
                buck2_data::TransitiveSetProjectionDef {
                    tset_id: 0,
                    producers: Some(buck2_data::ActionGraphProducers {
                        span_ids: vec![10],
                        ..Default::default()
                    }),
                    display_key: "TransitiveSetProjection(demo)".to_owned(),
                    ..Default::default()
                },
            )),
        ));
    }
    // t2: hybrid race, local loser + remote winner, with stages.
    frames.push(event(105_000, 20, 1, span_start(action_start(2, "link"))));
    frames.push(event(
        110_000,
        21,
        20,
        span_start(buck2_data::span_start_event::Data::ExecutorStage(
            buck2_data::ExecutorStageStart {
                stage: Some(buck2_data::executor_stage_start::Stage::Re(
                    buck2_data::ReStage {
                        stage: Some(buck2_data::re_stage::Stage::Queue(
                            buck2_data::ReQueue::default(),
                        )),
                    },
                )),
            },
        )),
    ));
    frames.push(event(
        130_000,
        21,
        20,
        span_end(
            20_000,
            buck2_data::span_end_event::Data::ExecutorStage(
                buck2_data::ExecutorStageEnd::default(),
            ),
        ),
    ));
    frames.push(event(
        106_000,
        22,
        20,
        span_start(buck2_data::span_start_event::Data::ReUpload(
            buck2_data::ReUploadStart::default(),
        )),
    ));
    frames.push(event(
        108_000,
        22,
        20,
        span_end(
            2_000,
            // An upload with nothing to send: every phase measured as zero.
            buck2_data::span_end_event::Data::ReUpload(buck2_data::ReUploadEnd {
                stats: Some(buck2_data::ReUploadStats::default()),
                bytes_uploaded: Some(0),
                ..Default::default()
            }),
        ),
    ));
    let remote_details = buck2_data::CommandExecutionDetails {
        signed_exit_code: Some(0),
        command_kind: Some(buck2_data::CommandExecutionKind {
            command: Some(buck2_data::command_execution_kind::Command::RemoteCommand(
                buck2_data::RemoteCommand {
                    action_digest: "abc:123".to_owned(),
                    queue_time: Some(dur(25_000)),
                    re_worker: Some(Box::new(buck2_data::ReWorkerDetails {
                        elastic_capacity: true,
                        cpu_time_user_us: Some(60_000),
                        cpu_time_system_us: Some(10_000),
                        input_download_bytes: Some(4096),
                        output_upload_bytes: Some(512),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            )),
        }),
        // The two client spans (30 ms and 100 ms) fit inside the 200 ms
        // action together, so only their client starts can show that
        // they overlapped.
        metadata: Some(buck2_data::CommandExecutionMetadata {
            wall_time: Some(dur(100_000)),
            client_attempt: Some(buck2_data::ClientAttempt {
                start_time: Some(ts(110_000)),
            }),
            execution_time: Some(dur(80_000)),
            execution_stats: Some(buck2_data::CommandExecutionStats {
                memory_peak: Some(1 << 20),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let cancelled_local = buck2_data::CommandExecution {
        details: Some(buck2_data::CommandExecutionDetails {
            metadata: Some(buck2_data::CommandExecutionMetadata {
                wall_time: Some(dur(30_000)),
                start_time: Some(ts(120_000)),
                client_attempt: Some(buck2_data::ClientAttempt {
                    start_time: Some(ts(120_000)),
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
        status: Some(buck2_data::command_execution::Status::Cancelled(
            buck2_data::command_execution::Cancelled {},
        )),
        ..Default::default()
    };
    frames.push(event(
        305_000,
        20,
        1,
        span_end(
            200_000,
            buck2_data::span_end_event::Data::ActionExecution(Box::new(
                buck2_data::ActionExecutionEnd {
                    key: Some(action_key(2)),
                    kind: 3,
                    name: Some(buck2_data::ActionName {
                        category: "link".to_owned(),
                        identifier: String::new(),
                    }),
                    execution_kind: 2, // remote
                    commands: vec![
                        cancelled_local,
                        buck2_data::CommandExecution {
                            details: Some(remote_details),
                            status: Some(buck2_data::command_execution::Status::Success(
                                buck2_data::command_execution::Success {},
                            )),
                            ..Default::default()
                        },
                    ],
                    wall_time: Some(dur(200_000)),
                    output_size: 1 << 16,
                    input_files_bytes: Some(1 << 22),
                    graph_producers: Some(buck2_data::ActionGraphProducers {
                        span_ids: vec![10],
                        tset_ids: vec![0],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        ),
    ));
    frames.push(event(
        306_000,
        0,
        0,
        instant(buck2_data::instant_event::Data::Snapshot(Box::new(
            buck2_data::Snapshot {
                buck2_max_rss: 1 << 30,
                ..Default::default()
            },
        ))),
    ));
    if let Some(spec) = footer {
        frames.push(event(
            307_000,
            0,
            0,
            instant(buck2_data::instant_event::Data::ActionGraphFooter(
                buck2_data::ActionGraphFooter {
                    schema: spec.schema,
                    incomplete: if spec.unfinished_walk {
                        Some(buck2_data::action_graph_footer::Incomplete::UnfinishedWalk as i32)
                    } else {
                        spec.nonfinal.then_some(
                            buck2_data::action_graph_footer::Incomplete::BuildFailed as i32,
                        )
                    },
                    action_ends: spec.action_ends,
                    tset_def_events: spec.tset_def_events,
                    structure_defs: spec.structure_defs,
                    aliases: spec.aliases,
                    ..Default::default()
                },
            )),
        ));
    }
    if with_command_end {
        frames.push(event(
            308_000,
            1,
            0,
            span_end(
                308_000,
                buck2_data::span_end_event::Data::Command(buck2_data::CommandEnd {
                    build_result: Some(buck2_data::BuildResult {
                        build_completed: true,
                    }),
                    data: Some(buck2_data::command_end::Data::Build(
                        buck2_data::BuildCommandEnd {
                            unresolved_target_patterns: vec![buck2_data::TargetPattern {
                                value: "fbcode//demo/...".to_owned(),
                            }],
                        },
                    )),
                    ..Default::default()
                }),
            ),
        ));
    }
    if with_result {
        frames.push(frame(buck2_cli_proto::command_progress::Progress::Result(
            Box::new(buck2_cli_proto::CommandResult {
                result: Some(buck2_cli_proto::command_result::Result::BuildResponse(
                    buck2_cli_proto::BuildResponse {
                        build_targets: vec![buck2_cli_proto::BuildTarget {
                            target: "fbcode//demo:t2".to_owned(),
                            ..Default::default()
                        }],
                        project_root: "/repo".to_owned(),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }),
        )));
    }
    let path = dir.join("demo_events.pb");
    std::fs::write(&path, frames.concat()).expect("test fixture dir must be writable");
    path
}

/// Mutate producer records, not the projected Scan: regression tests must
/// exercise both compaction and ordered reduction of the same wire evidence.
#[cfg(test)]
pub(super) fn rewrite(
    path: &Path,
    change: impl FnOnce(&mut Vec<buck2_cli_proto::CommandProgress>),
) {
    let input = std::fs::read(path).unwrap();
    let mut input = input.as_slice();
    let invocation = buck2_data::Invocation::decode_length_delimited(&mut input).unwrap();
    let mut records = Vec::new();
    while !input.is_empty() {
        records
            .push(buck2_cli_proto::CommandProgress::decode_length_delimited(&mut input).unwrap());
    }
    change(&mut records);
    let mut output = invocation.encode_length_delimited_to_vec();
    for record in records {
        record.encode_length_delimited(&mut output).unwrap();
    }
    std::fs::write(path, output).unwrap();
}

/// Standards-compliant Zstandard raw blocks, without an extra test-only
/// encoder dependency. Omit the final empty block to exercise truncated EOF
/// after every record has been delivered by the real decompressor.
#[cfg(test)]
pub(super) fn zstd_copy(path: &Path, complete: bool) -> PathBuf {
    let input = std::fs::read(path).unwrap();
    // Frame magic, single-segment descriptor and four-byte content size.
    let mut output = vec![0x28, 0xb5, 0x2f, 0xfd, 0xa0];
    output.extend(u32::try_from(input.len()).unwrap().to_le_bytes());
    for block in input.chunks(128 * 1024) {
        let header = (u32::try_from(block.len()).unwrap() << 3).to_le_bytes();
        output.extend(&header[..3]);
        output.extend(block);
    }
    if complete {
        output.extend([1, 0, 0]);
    }
    let target = path.with_extension(if complete {
        "complete.pb.zst"
    } else {
        "truncated.pb.zst"
    });
    std::fs::write(&target, output).unwrap();
    target
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StructureCase {
    Complete,
    NoTouch,
    MissingChunk,
    AliasCycle,
    Incomplete,
    ContradictoryDefinition,
    InputsOmitted,
}

/// Current A -> reused structural B -> current C, with a tset and an
/// explicitly resolved output-key alias between C and B. B has no execution.
/// `InputsOmitted` records B by name only, as the recorder does when nothing
/// that ran lies below it.
#[cfg(test)]
pub(super) fn structural_log(dir: &Path, case: StructureCase) -> PathBuf {
    use buck2_data::buck_event::Data as Event;
    use buck2_data::instant_event::Data as Instant;
    use buck2_data::span_end_event::Data as End;
    let path = demo_log(dir, Some(1), true, true);
    rewrite(&path, |records| {
        if case == StructureCase::NoTouch {
            records.retain(|record| {
                let Some(buck2_cli_proto::command_progress::Progress::Event(event)) =
                    &record.progress
                else {
                    return true;
                };
                !matches!(
                    &event.data,
                    Some(Event::SpanStart(buck2_data::SpanStartEvent {
                        data: Some(buck2_data::span_start_event::Data::ActionExecution(_)),
                    })) | Some(Event::SpanEnd(buck2_data::SpanEndEvent {
                        data: Some(End::ActionExecution(_)),
                        ..
                    }))
                )
            });
        }
        for record in records.iter_mut() {
            let Some(buck2_cli_proto::command_progress::Progress::Event(event)) =
                &mut record.progress
            else {
                continue;
            };
            match &mut event.data {
                Some(Event::SpanEnd(end)) => {
                    if let Some(End::ActionExecution(action)) = &mut end.data
                        && action.key.as_ref() == Some(&action_key(2))
                        && let Some(producers) = &mut action.graph_producers
                    {
                        producers.span_ids.clear();
                    }
                }
                Some(Event::Instant(instant)) => match &mut instant.data {
                    Some(Instant::TransitiveSetProjectionDef(def)) => {
                        let producers = def.producers.get_or_insert_default();
                        producers.span_ids.clear();
                        producers.reused_ids = vec![4];
                    }
                    Some(Instant::ActionGraphFooter(footer)) => {
                        if case == StructureCase::NoTouch {
                            footer.action_ends = 0;
                        }
                        footer.structure_defs = match case {
                            StructureCase::AliasCycle => 0,
                            StructureCase::MissingChunk => 2,
                            _ => 1,
                        };
                        footer.aliases = if case == StructureCase::AliasCycle {
                            2
                        } else {
                            1
                        };
                        if case == StructureCase::Incomplete {
                            footer.incomplete = Some(
                                buck2_data::action_graph_footer::Incomplete::UnfinishedWalk as i32,
                            );
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        let mut extra = Vec::new();
        if case != StructureCase::AliasCycle {
            extra.push(Instant::ActionGraphStructureDef(
                buck2_data::ActionGraphStructureDef {
                    reused_id: 3,
                    key: Some(action_key(
                        if case == StructureCase::ContradictoryDefinition {
                            1
                        } else {
                            3
                        },
                    )),
                    producers: Some(buck2_data::ActionGraphProducers {
                        span_ids: if matches!(
                            case,
                            StructureCase::NoTouch | StructureCase::InputsOmitted
                        ) {
                            vec![]
                        } else {
                            vec![10]
                        },
                        ..Default::default()
                    }),
                    inputs_omitted: case == StructureCase::InputsOmitted,
                    ..Default::default()
                },
            ));
        }
        extra.push(Instant::ActionGraphAlias(buck2_data::ActionGraphAlias {
            id: 4,
            key: Some(action_key(4)),
            canonical: Some(buck2_data::action_graph_alias::Canonical::ReusedId(3)),
        }));
        if case == StructureCase::AliasCycle {
            extra.push(Instant::ActionGraphAlias(buck2_data::ActionGraphAlias {
                id: 3,
                key: Some(action_key(3)),
                canonical: Some(buck2_data::action_graph_alias::Canonical::ReusedId(4)),
            }));
        }
        // Intentionally before the current producer end: these identity-only
        // records must survive missing timestamps and resolve after the scan.
        for data in extra.into_iter().rev() {
            records.insert(
                0,
                buck2_cli_proto::CommandProgress {
                    progress: Some(buck2_cli_proto::command_progress::Progress::Event(
                        Box::new(buck2_data::BuckEvent {
                            trace_id: RUN_ID.to_owned(),
                            data: Some(instant(data)),
                            ..Default::default()
                        }),
                    )),
                },
            );
        }
    });
    path
}

/// A structural capture, or with `empty` a command where nothing ran, which records no
/// definitions.
#[cfg(test)]
pub(super) fn activity_log(dir: &Path, empty: bool) -> PathBuf {
    use buck2_cli_proto::command_progress::Progress;
    use buck2_data::buck_event::Data;
    use buck2_data::instant_event::Data as Instant;
    let case = if empty {
        StructureCase::NoTouch
    } else {
        StructureCase::Complete
    };
    let path = structural_log(dir, case);
    rewrite(&path, |records| {
        records.retain_mut(|record| {
            let Some(Progress::Event(event)) = &mut record.progress else {
                return true;
            };
            let Some(Data::Instant(instant)) = &mut event.data else {
                return true;
            };
            match &mut instant.data {
                Some(Instant::ActionGraphFooter(footer)) => {
                    footer.definitions_walk_us = 123;
                    if empty {
                        footer.tset_def_events = 0;
                        footer.structure_defs = 0;
                        footer.aliases = 0;
                    }
                }
                Some(
                    Instant::TransitiveSetProjectionDef(_)
                    | Instant::ActionGraphStructureDef(_)
                    | Instant::ActionGraphAlias(_),
                ) if empty => return false,
                _ => {}
            }
            true
        });
    });
    path
}

/// Two actions of one kind that differ only in how they were served:
/// `t1` came out of the action cache, `t2` ran locally.
pub fn cache_hit_log(dir: &Path) -> PathBuf {
    let mut frames: Vec<Vec<u8>> = Vec::new();
    frames.push(
        buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned(), "build".to_owned()],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
    );
    for (n, execution_kind) in [(1u8, 3i32), (2, 1)] {
        let span = u64::from(n) * 10;
        let start_us = i64::from(n) * 1_000;
        frames.push(event(
            start_us,
            span,
            1,
            span_start(action_start(n, "rustc")),
        ));
        frames.push(event(
            start_us + 500,
            span,
            1,
            span_end(
                500,
                buck2_data::span_end_event::Data::ActionExecution(Box::new(
                    buck2_data::ActionExecutionEnd {
                        key: Some(action_key(n)),
                        kind: 3,
                        name: Some(buck2_data::ActionName {
                            category: "rustc".to_owned(),
                            identifier: String::new(),
                        }),
                        execution_kind,
                        output_size: 4096,
                        ..Default::default()
                    },
                )),
            ),
        ));
    }
    frames.push(event(
        10_000,
        2,
        0,
        span_end(
            10_000,
            buck2_data::span_end_event::Data::Command(buck2_data::CommandEnd {
                build_result: Some(buck2_data::BuildResult {
                    build_completed: true,
                }),
                data: Some(buck2_data::command_end::Data::Build(Default::default())),
                ..Default::default()
            }),
        ),
    ));
    let path = dir.join("cache_events.pb");
    std::fs::write(&path, frames.concat()).unwrap();
    path
}

/// Local checks, remote validation, cancellation, overlap, zero, and an unknown
/// future outcome. Bad pairs and open starts must never invent observations.
#[cfg(test)]
pub fn local_cache_stage_log(dir: &Path) -> PathBuf {
    use buck2_data::span_end_event::Data as End;
    use buck2_data::span_start_event::Data as Start;
    let mut frames = vec![
        buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned(), "build".to_owned()],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
        event(0, 10, 1, span_start(action_start(1, "local-cache-fixture"))),
    ];
    for (sid, at, elapsed, filtered, remote, outcome) in [
        (11, 0, 30, false, false, Some(2)),
        (12, 20, 30, true, false, Some(1)),
        (13, 0, 90, true, true, Some(0)),
        (14, 80, 10, false, false, None),
        (15, 90, 10, false, false, Some(3)),
        (16, 100, 0, false, false, Some(42)),
    ] {
        frames.push(event(
            at,
            sid,
            10,
            span_start(Start::MatchDepFiles(buck2_data::MatchDepFilesStart {
                checking_filtered_inputs: filtered,
                remote_cache: remote,
            })),
        ));
        let end = if let Some(outcome) = outcome {
            End::MatchDepFiles(buck2_data::MatchDepFilesEnd {
                outcome,
                persisted_probe_us: (sid == 15).then_some(0),
                persisted_fetch_us: (sid == 15).then_some(5),
                persisted_fetches: (sid == 15).then_some(2),
            })
        } else {
            End::SpanCancelled(Default::default())
        };
        frames.push(event(at + elapsed, sid, 10, span_end(elapsed, end)));
    }
    // No matching start; wrong end consumes the start; no end at EOF.
    frames.push(event(
        60,
        17,
        10,
        span_end(10, End::MatchDepFiles(Default::default())),
    ));
    frames.push(event(
        70,
        18,
        10,
        span_start(Start::MatchDepFiles(Default::default())),
    ));
    frames.push(event(
        80,
        18,
        10,
        span_end(10, End::ExecutorStage(Default::default())),
    ));
    frames.push(event(
        90,
        18,
        10,
        span_end(20, End::MatchDepFiles(Default::default())),
    ));
    // Generic/empty ends, the normalized ActionExecution-end path, and
    // missing timestamps must consume the SID without inventing a stage.
    for (sid, timestamp, data) in [
        (20, Some(ts(80)), Some(End::Fake(Default::default()))),
        (21, Some(ts(80)), None),
        (22, Some(ts(80)), Some(End::ActionExecution(Box::default()))),
        (23, None, Some(End::MatchDepFiles(Default::default()))),
    ] {
        frames.push(event(
            70,
            sid,
            10,
            span_start(Start::MatchDepFiles(Default::default())),
        ));
        frames.push(frame(buck2_cli_proto::command_progress::Progress::Event(
            Box::new(buck2_data::BuckEvent {
                timestamp,
                trace_id: RUN_ID.to_owned(),
                span_id: sid,
                parent_id: 10,
                data: Some(buck2_data::buck_event::Data::SpanEnd(
                    buck2_data::SpanEndEvent {
                        duration: Some(dur(10)),
                        data,
                        ..Default::default()
                    },
                )),
            }),
        )));
        frames.push(event(
            90,
            sid,
            10,
            span_end(20, End::MatchDepFiles(Default::default())),
        ));
    }
    frames.push(event(
        95,
        19,
        10,
        span_start(Start::MatchDepFiles(Default::default())),
    ));
    frames.push(event(
        100,
        10,
        1,
        span_end(
            100,
            End::ActionExecution(Box::new(buck2_data::ActionExecutionEnd {
                key: Some(action_key(1)),
                kind: 3,
                execution_kind: buck2_data::ActionExecutionKind::LocalActionCache as i32,
                ..Default::default()
            })),
        ),
    ));
    frames.push(frame(buck2_cli_proto::command_progress::Progress::Result(
        Box::new(buck2_cli_proto::CommandResult {
            result: Some(buck2_cli_proto::command_result::Result::BuildResponse(
                Default::default(),
            )),
            ..Default::default()
        }),
    )));
    let path = dir.join("local_cache_stages.pb");
    std::fs::write(&path, frames.concat()).unwrap();
    path
}

/// An all-hit action with existing stage intervals and optional newer phase
/// payloads. The false case writes the historical ExecutorStageEnd wire shape.
#[cfg(test)]
pub fn cache_stage_log(dir: &Path, timings: bool) -> PathBuf {
    use buck2_data::executor_stage_start::Stage;
    use buck2_data::span_end_event::Data as End;
    use buck2_data::span_start_event::Data as Start;
    let path = cache_hit_log(dir);
    let query = buck2_data::ExecutorStageEnd {
        cache_query: timings.then_some(buck2_data::CacheQueryTimings {
            slot_wait_us: 0,
            remote_lookup_us: 250,
        }),
        ..Default::default()
    };
    let hit = buck2_data::ExecutorStageEnd {
        cache_hit: timings.then_some(buck2_data::CacheHitTimings {
            tree_admission_us: None,
            tree_fetch_us: Some(5),
            local_bookkeeping_us: Some(10),
            declare_us: Some(150),
            tree_count: 2,
        }),
        ..Default::default()
    };
    let mut stages = Vec::new();
    for (sid, at, elapsed, stage, end) in [
        (
            101,
            1_000,
            5,
            Stage::Prepare(Default::default()),
            Default::default(),
        ),
        (
            102,
            1_005,
            300,
            Stage::CacheQuery(Default::default()),
            query,
        ),
        (103, 1_305, 190, Stage::CacheHit(Default::default()), hit),
    ] {
        for bytes in [
            event(
                at,
                sid,
                10,
                span_start(Start::ExecutorStage(buck2_data::ExecutorStageStart {
                    stage: Some(stage),
                })),
            ),
            event(
                at + elapsed,
                sid,
                10,
                span_end(elapsed, End::ExecutorStage(end)),
            ),
        ] {
            stages.push(
                buck2_cli_proto::CommandProgress::decode_length_delimited(bytes.as_slice())
                    .unwrap(),
            );
        }
    }
    rewrite(&path, |records| {
        records.splice(1..1, stages);
    });
    path
}

/// A final command with 900 ms of stored execution metadata inside a
/// 100 ms action. A preceding local attempt is optional; overlapping
/// Local.Execute/WorkerExecute spans cover 30 ms, not 50 ms. Remote's
/// enum-default cache type deliberately exercises older payloads where
/// only an affirmative cache_hit bool establishes reuse.
#[cfg(test)]
pub fn cached_execution_log(
    dir: &Path,
    execution_kind: i32,
    remote_cache_hit: bool,
    with_local_attempt: bool,
) -> PathBuf {
    let mut frames = vec![
        buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned(), "build".to_owned()],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
        event(1_000, 10, 1, span_start(action_start(1, "rustc"))),
    ];
    let mut commands = Vec::new();
    if with_local_attempt {
        for (sid, start, elapsed, stage) in [
            (
                11,
                10_000,
                30_000,
                buck2_data::local_stage::Stage::Execute(buck2_data::LocalExecute::default()),
            ),
            (
                12,
                15_000,
                20_000,
                buck2_data::local_stage::Stage::WorkerExecute(buck2_data::WorkerExecute::default()),
            ),
        ] {
            frames.push(event(
                start,
                sid,
                10,
                span_start(buck2_data::span_start_event::Data::ExecutorStage(
                    buck2_data::ExecutorStageStart {
                        stage: Some(buck2_data::executor_stage_start::Stage::Local(
                            buck2_data::LocalStage { stage: Some(stage) },
                        )),
                    },
                )),
            ));
            frames.push(event(
                start + elapsed,
                sid,
                10,
                span_end(
                    elapsed,
                    buck2_data::span_end_event::Data::ExecutorStage(
                        buck2_data::ExecutorStageEnd::default(),
                    ),
                ),
            ));
        }
        let mut prior = local_success(&["rustc", "lib.rs"], 30_000);
        prior.status = Some(buck2_data::command_execution::Status::Cancelled(
            buck2_data::command_execution::Cancelled::default(),
        ));
        commands.push(prior);
    }
    let mut winner = local_success(&[], 900_000);
    let details = winner.details.as_mut().unwrap();
    details.command_kind = Some(buck2_data::CommandExecutionKind {
        command: Some(buck2_data::command_execution_kind::Command::RemoteCommand(
            buck2_data::RemoteCommand {
                cache_hit: remote_cache_hit,
                ..Default::default()
            },
        )),
    });
    details.metadata.as_mut().unwrap().wall_time = Some(dur(5_000));
    commands.push(winner);
    frames.push(event(
        101_000,
        10,
        1,
        span_end(
            100_000,
            buck2_data::span_end_event::Data::ActionExecution(Box::new(
                buck2_data::ActionExecutionEnd {
                    key: Some(action_key(1)),
                    kind: 3,
                    execution_kind,
                    commands,
                    ..Default::default()
                },
            )),
        ),
    ));
    frames.push(event(
        102_000,
        2,
        0,
        span_end(
            102_000,
            buck2_data::span_end_event::Data::Command(buck2_data::CommandEnd {
                build_result: Some(buck2_data::BuildResult {
                    build_completed: true,
                }),
                data: Some(buck2_data::command_end::Data::Build(Default::default())),
                ..Default::default()
            }),
        ),
    ));
    let path = dir.join("cached_execution_events.pb");
    std::fs::write(&path, frames.concat()).unwrap();
    path
}

/// The two client-state patterns observed in the September 10 isolated
/// build, reduced to one action and exact measured stage durations. No
/// worker log is synthesized: the remote retry is visible as a second
/// QUEUED/download state within one command result.
#[cfg(test)]
#[derive(Clone, Copy)]
pub enum WaitStateCase {
    LocalWinner { download_end: bool },
    RemoteRetry,
}

#[cfg(test)]
pub fn wait_state_log(dir: &Path, case: WaitStateCase) -> PathBuf {
    use buck2_data::executor_stage_start::Stage;
    use buck2_data::local_stage::Stage as L;
    use buck2_data::re_stage::Stage as R;
    use buck2_data::span_end_event::Data as End;
    use buck2_data::span_start_event::Data as Start;

    let mut frames = vec![
        buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned(), "build".to_owned()],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
        event(0, 10, 1, span_start(action_start(1, "wait-fixture"))),
    ];
    let mut sid = 11;
    let mut add = |start: i64, elapsed: i64, name: &str, end: Option<bool>| {
        let stage = match name {
            "Prepare" => Stage::Prepare(Default::default()),
            "CacheQuery" => Stage::CacheQuery(Default::default()),
            "Re.Queue" => Stage::Re(buck2_data::ReStage {
                stage: Some(R::Queue(Default::default())),
            }),
            "Re.WorkerDownload" => Stage::Re(buck2_data::ReStage {
                stage: Some(R::WorkerDownload(Default::default())),
            }),
            "Re.WorkerUpload" => Stage::Re(buck2_data::ReStage {
                stage: Some(R::WorkerUpload(Default::default())),
            }),
            "Local.MaterializeInputs" => Stage::Local(buck2_data::LocalStage {
                stage: Some(L::MaterializeInputs(Default::default())),
            }),
            "Local.PrepareOutputs" => Stage::Local(buck2_data::LocalStage {
                stage: Some(L::PrepareOutputs(Default::default())),
            }),
            "Local.Execute" => Stage::Local(buck2_data::LocalStage {
                stage: Some(L::Execute(Default::default())),
            }),
            "ReUpload" | "ReSubmit" => Stage::Prepare(Default::default()),
            _ => panic!("unknown fixture stage {name}"),
        };
        let start_payload = match name {
            "ReUpload" => Start::ReUpload(Default::default()),
            "ReSubmit" => Start::ReSubmit(Default::default()),
            _ => Start::ExecutorStage(buck2_data::ExecutorStageStart { stage: Some(stage) }),
        };
        frames.push(event(start, sid, 10, span_start(start_payload)));
        if let Some(cancelled) = end {
            let end_payload = if cancelled {
                End::SpanCancelled(Default::default())
            } else {
                match name {
                    "ReUpload" => End::ReUpload(Default::default()),
                    "ReSubmit" => End::ReSubmit(Default::default()),
                    _ => End::ExecutorStage(Default::default()),
                }
            };
            frames.push(event(
                start + elapsed,
                sid,
                10,
                span_end(elapsed, end_payload),
            ));
        }
        sid += 1;
    };
    let (wall, execution_kind, command, complete) = match case {
        WaitStateCase::LocalWinner { download_end } => {
            add(1_109, 15, "Prepare", Some(false));
            add(1_131, 96_313, "CacheQuery", Some(false));
            add(97_473, 518_509, "ReUpload", Some(false));
            add(616_017, 2_414_923, "ReSubmit", Some(false));
            add(3_030_953, 118_342, "Re.Queue", Some(false));
            add(
                3_149_358,
                579_439_598,
                "Re.WorkerDownload",
                download_end.then_some(true),
            );
            add(
                551_891_486,
                11_200_744,
                "Local.MaterializeInputs",
                Some(false),
            );
            add(563_092_354, 9_495_270, "Local.PrepareOutputs", Some(false));
            add(572_587_641, 2_809_091, "Local.Execute", Some(false));
            let mut command = local_success(&["rustc"], 2_809_086);
            command
                .details
                .as_mut()
                .unwrap()
                .metadata
                .as_mut()
                .unwrap()
                .input_materialization_duration = Some(dur(11_200_743));
            (582_589_770, 1, command, download_end)
        }
        WaitStateCase::RemoteRetry => {
            add(36_140_238, 94_947, "Re.Queue", Some(false));
            add(36_236_107, 1_577_896_801, "Re.WorkerDownload", Some(false));
            add(1_614_132_913, 11_788, "Re.Queue", Some(false));
            add(1_614_144_765, 850_932, "Re.WorkerDownload", Some(false));
            add(1_621_027_080, 1_208_061, "Re.WorkerUpload", Some(false));
            let mut command = local_success(&[], 6_029_947);
            let details = command.details.as_mut().unwrap();
            details.command_kind = Some(buck2_data::CommandExecutionKind {
                command: Some(buck2_data::command_execution_kind::Command::RemoteCommand(
                    buck2_data::RemoteCommand {
                        cache_hit: false,
                        cache_hit_type: buck2_data::RemoteCommandOrigin::Executed as i32,
                        queue_time: Some(dur(106_730)),
                        ..Default::default()
                    },
                )),
            });
            let metadata = details.metadata.as_mut().unwrap();
            metadata.wall_time = Some(dur(1_587_579_884));
            metadata.queue_duration = Some(dur(1_577_954_967));
            metadata.input_materialization_duration = Some(dur(850_809));
            (1_622_235_649, 2, command, true)
        }
    };
    frames.push(event(
        wall,
        10,
        1,
        span_end(
            wall,
            End::ActionExecution(Box::new(buck2_data::ActionExecutionEnd {
                key: Some(action_key(1)),
                kind: 3,
                execution_kind,
                commands: vec![command],
                ..Default::default()
            })),
        ),
    ));
    if complete {
        frames.push(frame(buck2_cli_proto::command_progress::Progress::Result(
            Box::new(buck2_cli_proto::CommandResult {
                result: Some(buck2_cli_proto::command_result::Result::BuildResponse(
                    buck2_cli_proto::BuildResponse::default(),
                )),
                ..Default::default()
            }),
        )));
    }
    let path = dir.join("wait_states.pb");
    std::fs::write(&path, frames.concat()).unwrap();
    path
}

/// A complete two-action log whose SCAN order differs from its
/// timestamp order: the consumer `t2` (started 150ms, dep on `t1` by
/// span id) closes its span first, the producer `t1` (started 1ms)
/// second.  The lane sort's permutation of the event list is
/// therefore not the identity — the shape that exposed the graph
/// row-name desynchronisation.  Every other log here has the two
/// orders equal, which is why they could never catch it.
pub fn reorder_log(dir: &Path) -> PathBuf {
    let mut frames: Vec<Vec<u8>> = vec![
        buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned(), "build".to_owned()],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
    ];
    frames.push(event(
        500,
        0,
        0,
        instant(buck2_data::instant_event::Data::ActionGraphStart(
            buck2_data::ActionGraphStart { schema: 1 },
        )),
    ));
    frames.push(event(150_000, 20, 1, span_start(action_start(2, "link"))));
    frames.push(event(
        200_000,
        20,
        1,
        span_end(
            50_000,
            buck2_data::span_end_event::Data::ActionExecution(Box::new(
                buck2_data::ActionExecutionEnd {
                    key: Some(action_key(2)),
                    kind: 3,
                    name: Some(buck2_data::ActionName {
                        category: "link".to_owned(),
                        identifier: String::new(),
                    }),
                    execution_kind: 1,
                    commands: vec![local_success(&["ld", "t2"], 40_000)],
                    graph_producers: Some(buck2_data::ActionGraphProducers {
                        span_ids: vec![10],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        ),
    ));
    frames.push(event(1_000, 10, 1, span_start(action_start(1, "rustc"))));
    frames.push(event(
        201_000,
        10,
        1,
        span_end(
            200_000,
            buck2_data::span_end_event::Data::ActionExecution(Box::new(
                buck2_data::ActionExecutionEnd {
                    key: Some(action_key(1)),
                    kind: 3,
                    name: Some(buck2_data::ActionName {
                        category: "rustc".to_owned(),
                        identifier: String::new(),
                    }),
                    execution_kind: 1,
                    commands: vec![local_success(&["rustc", "lib.rs"], 90_000)],
                    ..Default::default()
                },
            )),
        ),
    ));
    frames.push(event(
        300_000,
        0,
        0,
        instant(buck2_data::instant_event::Data::ActionGraphFooter(
            buck2_data::ActionGraphFooter {
                schema: 1,
                action_ends: 2,
                ..Default::default()
            },
        )),
    ));
    frames.push(event(
        301_000,
        2,
        0,
        span_end(
            301_000,
            buck2_data::span_end_event::Data::Command(buck2_data::CommandEnd {
                build_result: Some(buck2_data::BuildResult {
                    build_completed: true,
                }),
                data: Some(buck2_data::command_end::Data::Build(Default::default())),
                ..Default::default()
            }),
        ),
    ));
    frames.push(frame(buck2_cli_proto::command_progress::Progress::Result(
        Box::new(buck2_cli_proto::CommandResult {
            result: Some(buck2_cli_proto::command_result::Result::BuildResponse(
                buck2_cli_proto::BuildResponse::default(),
            )),
            ..Default::default()
        }),
    )));
    let path = dir.join("reorder_events.pb");
    std::fs::write(&path, frames.concat()).expect("test fixture dir must be writable");
    path
}

/// One action over a diamond of eight tset defs, four of which share
/// one display key.  Walking the join-row worklist their children seed
/// is where map iteration order could reach the bytes of `graph.json`,
/// and the shared key pins that a label is never an identity: four
/// defs, four rows.  Complete log, footer schema 1.
pub fn many_tsets_log(dir: &Path) -> PathBuf {
    let mut frames: Vec<Vec<u8>> = vec![
        buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned(), "build".to_owned()],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
    ];
    frames.push(event(
        500,
        0,
        0,
        instant(buck2_data::instant_event::Data::ActionGraphStart(
            buck2_data::ActionGraphStart { schema: 1 },
        )),
    ));
    frames.push(event(1_000, 10, 1, span_start(action_start(1, "rustc"))));
    frames.push(event(
        101_000,
        10,
        1,
        span_end(
            100_000,
            buck2_data::span_end_event::Data::ActionExecution(Box::new(
                buck2_data::ActionExecutionEnd {
                    key: Some(action_key(1)),
                    kind: 3,
                    name: Some(buck2_data::ActionName {
                        category: "rustc".to_owned(),
                        identifier: String::new(),
                    }),
                    execution_kind: 1,
                    commands: vec![local_success(&["rustc", "lib.rs"], 90_000)],
                    graph_producers: Some(buck2_data::ActionGraphProducers {
                        tset_ids: vec![0],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        ),
    ));
    let def = |id: u64, key: &str, child_ids: Vec<u64>| {
        instant(buck2_data::instant_event::Data::TransitiveSetProjectionDef(
            buck2_data::TransitiveSetProjectionDef {
                tset_id: id,
                producers: Some(buck2_data::ActionGraphProducers {
                    tset_ids: child_ids,
                    ..Default::default()
                }),
                display_key: format!("TransitiveSetProjection({key})"),
                ..Default::default()
            },
        ))
    };
    frames.push(event(2_000, 0, 0, def(0, "a", vec![1, 3, 5, 6])));
    for (id, child) in [(1, 2), (3, 4), (6, 7)] {
        frames.push(event(2_000, 0, 0, def(id, "b", vec![child])));
    }
    frames.push(event(2_000, 0, 0, def(5, "b", vec![])));
    frames.push(event(2_000, 0, 0, def(2, "c", vec![])));
    frames.push(event(2_000, 0, 0, def(4, "d", vec![])));
    frames.push(event(2_000, 0, 0, def(7, "e", vec![])));
    frames.push(event(
        300_000,
        0,
        0,
        instant(buck2_data::instant_event::Data::ActionGraphFooter(
            buck2_data::ActionGraphFooter {
                schema: 1,
                action_ends: 1,
                tset_def_events: 8,
                ..Default::default()
            },
        )),
    ));
    frames.push(event(
        301_000,
        2,
        0,
        span_end(
            301_000,
            buck2_data::span_end_event::Data::Command(buck2_data::CommandEnd {
                is_success: true,
                ..Default::default()
            }),
        ),
    ));
    frames.push(frame(buck2_cli_proto::command_progress::Progress::Result(
        Box::new(buck2_cli_proto::CommandResult {
            result: Some(buck2_cli_proto::command_result::Result::BuildResponse(
                buck2_cli_proto::BuildResponse::default(),
            )),
            ..Default::default()
        }),
    )));
    let path = dir.join("many_tsets_events.pb");
    std::fs::write(&path, frames.concat()).expect("test fixture dir must be writable");
    path
}

/// Four actions and two projections whose display keys are identical
/// (the producer caps the rendering at 4 KiB, so deep dynamic-output
/// chains render alike).  `A` (10s) and `B` (1s) are owned by
/// projections 0 and 1 respectively; `C` (1s) depends on projection 0
/// and `D` (20s) on projection 1.  The true critical path is
/// `B -> D` = 21s; a reader that joined the two projections by label
/// would invent `A -> D` = 30s.  Complete log, footer schema 1.
pub fn equal_label_log(dir: &Path) -> PathBuf {
    let mut frames: Vec<Vec<u8>> = vec![
        buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned(), "build".to_owned()],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
    ];
    frames.push(event(
        500,
        0,
        0,
        instant(buck2_data::instant_event::Data::ActionGraphStart(
            buck2_data::ActionGraphStart { schema: 1 },
        )),
    ));
    // (action number, span id, start, duration, projection it depends on)
    let acts: [(u8, u64, i64, i64, Option<u64>); 4] = [
        (1, 10, 1_000, 10_000_000, None),
        (2, 20, 1_000, 1_000_000, None),
        (3, 30, 10_001_000, 1_000_000, Some(0)),
        (4, 40, 1_001_000, 20_000_000, Some(1)),
    ];
    for (n, sid, start, dur_us, dep) in acts {
        frames.push(event(start, sid, 1, span_start(action_start(n, "compile"))));
        frames.push(event(
            start + dur_us,
            sid,
            1,
            span_end(
                dur_us,
                buck2_data::span_end_event::Data::ActionExecution(Box::new(
                    buck2_data::ActionExecutionEnd {
                        key: Some(action_key(n)),
                        kind: 3,
                        name: Some(buck2_data::ActionName {
                            category: "compile".to_owned(),
                            identifier: String::new(),
                        }),
                        execution_kind: 1,
                        commands: vec![local_success(&["cc"], dur_us)],
                        graph_producers: Some(buck2_data::ActionGraphProducers {
                            tset_ids: dep.into_iter().collect(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )),
            ),
        ));
    }
    for (id, owner_sid) in [(0u64, 10u64), (1, 20)] {
        frames.push(event(
            2_000,
            0,
            0,
            instant(buck2_data::instant_event::Data::TransitiveSetProjectionDef(
                buck2_data::TransitiveSetProjectionDef {
                    tset_id: id,
                    producers: Some(buck2_data::ActionGraphProducers {
                        span_ids: vec![owner_sid],
                        ..Default::default()
                    }),
                    display_key: "TransitiveSetProjection(same truncated rendering...)".to_owned(),
                    ..Default::default()
                },
            )),
        ));
    }
    frames.push(event(
        22_000_000,
        0,
        0,
        instant(buck2_data::instant_event::Data::ActionGraphFooter(
            buck2_data::ActionGraphFooter {
                schema: 1,
                action_ends: 4,
                tset_def_events: 2,
                ..Default::default()
            },
        )),
    ));
    frames.push(event(
        22_001_000,
        2,
        0,
        span_end(
            22_001_000,
            buck2_data::span_end_event::Data::Command(buck2_data::CommandEnd {
                is_success: true,
                ..Default::default()
            }),
        ),
    ));
    frames.push(frame(buck2_cli_proto::command_progress::Progress::Result(
        Box::new(buck2_cli_proto::CommandResult {
            result: Some(buck2_cli_proto::command_result::Result::BuildResponse(
                buck2_cli_proto::BuildResponse::default(),
            )),
            ..Default::default()
        }),
    )));
    let path = dir.join("equal_label_events.pb");
    std::fs::write(&path, frames.concat()).expect("test fixture dir must be writable");
    path
}

/// A log torn before the footer: the invocation, the capture-start
/// stamp, one complete local action, then nothing — no footer, no
/// client `Result`.  The shape a build killed mid-flight leaves
/// behind, which is exactly what the start stamp exists for.
pub fn torn_log_with_start_stamp(dir: &Path, schema: u32) -> PathBuf {
    let mut frames: Vec<Vec<u8>> = vec![
        buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned(), "build".to_owned()],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
    ];
    frames.push(event(
        500,
        0,
        0,
        instant(buck2_data::instant_event::Data::ActionGraphStart(
            buck2_data::ActionGraphStart { schema },
        )),
    ));
    frames.push(event(1_000, 10, 1, span_start(action_start(1, "rustc"))));
    frames.push(event(
        101_000,
        10,
        1,
        span_end(
            100_000,
            buck2_data::span_end_event::Data::ActionExecution(Box::new(
                buck2_data::ActionExecutionEnd {
                    key: Some(action_key(1)),
                    kind: 3,
                    name: Some(buck2_data::ActionName {
                        category: "rustc".to_owned(),
                        identifier: String::new(),
                    }),
                    execution_kind: 1, // local
                    commands: vec![local_success(&["rustc", "lib.rs"], 90_000)],
                    output_size: 4096,
                    ..Default::default()
                },
            )),
        ),
    ));
    let path = dir.join("torn_events.pb");
    std::fs::write(&path, frames.concat()).expect("test fixture dir must be writable");
    path
}

/// Install lifecycle plus overlapping analyses, one action, sampled daemon
/// counters and Buck's separately clocked critical path. Deliberately no graph
/// footer: command success must not manufacture verified graph completeness.
pub fn command_profile_log(dir: &Path) -> PathBuf {
    use buck2_data::critical_path_entry2::Entry;
    use buck2_data::instant_event::Data as Instant;
    use buck2_data::span_end_event::Data as End;
    use buck2_data::span_start_event::Data as Start;
    let mut frames = vec![
        buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned(), "install".to_owned(), "fb4a".to_owned()],
            working_dir: "/repo".to_owned(),
            trace_id: Some(RUN_ID.to_owned()),
            start_time: Some(ts(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec(),
    ];
    frames.push(event(
        100,
        1,
        0,
        span_start(Start::Command(buck2_data::CommandStart {
            data: Some(buck2_data::command_start::Data::Install(Default::default())),
            ..Default::default()
        })),
    ));
    frames.push(event(
        110,
        2,
        1,
        span_start(Start::FileWatcher(Default::default())),
    ));
    frames.push(event(
        150,
        0,
        0,
        instant(Instant::Snapshot(Box::new(buck2_data::Snapshot {
            buck2_rss: Some(1000),
            buck2_max_rss: 9000,
            malloc_bytes_active: Some(500),
            buck2_user_cpu_us: 10,
            buck2_system_cpu_us: 5,
            re_download_bytes: 100,
            ..Default::default()
        }))),
    ));
    frames.push(event(
        160,
        2,
        1,
        span_end(50, End::FileWatcher(Default::default())),
    ));
    frames.push(event(
        200,
        3,
        1,
        span_start(Start::Analysis(Default::default())),
    ));
    frames.push(event(
        300,
        4,
        1,
        span_start(Start::Analysis(Default::default())),
    ));
    frames.push(event(400, 10, 3, span_start(action_start(1, "compile"))));
    frames.push(event(
        450,
        11,
        10,
        span_start(Start::ExecutorStage(buck2_data::ExecutorStageStart {
            stage: Some(buck2_data::executor_stage_start::Stage::Local(
                buck2_data::LocalStage {
                    stage: Some(buck2_data::local_stage::Stage::Execute(
                        buck2_data::LocalExecute {
                            command: Some(buck2_data::LocalCommand {
                                argv: vec!["cc".to_owned()],
                                ..Default::default()
                            }),
                        },
                    )),
                },
            )),
        })),
    ));
    frames.push(event(
        490,
        11,
        10,
        span_end(40, End::ExecutorStage(Default::default())),
    ));
    frames.push(event(
        500,
        10,
        3,
        span_end(
            100,
            End::ActionExecution(Box::new(buck2_data::ActionExecutionEnd {
                key: Some(action_key(1)),
                kind: 3,
                name: Some(buck2_data::ActionName {
                    category: "compile".to_owned(),
                    identifier: String::new(),
                }),
                execution_kind: 1,
                commands: vec![local_success(&["cc"], 40)],
                ..Default::default()
            })),
        ),
    ));
    frames.push(event(
        600,
        3,
        1,
        span_end(400, End::Analysis(Default::default())),
    ));
    frames.push(event(
        900,
        4,
        1,
        span_end(600, End::Analysis(Default::default())),
    ));
    frames.push(event(
        1000,
        5,
        1,
        span_start(Start::ConnectToInstaller(Default::default())),
    ));
    frames.push(event(
        1000,
        7,
        1,
        span_start(Start::Load(Default::default())),
    ));
    frames.push(event(
        1010,
        7,
        1,
        span_end(10, End::SpanCancelled(Default::default())),
    ));
    frames.push(event(
        1100,
        5,
        1,
        span_end(100, End::ConnectToInstaller(Default::default())),
    ));
    frames.push(event(
        1100,
        6,
        1,
        span_start(Start::InstallEventInfo(buck2_data::InstallEventInfoStart {
            artifact_name: "apk".to_owned(),
            file_path: "output.apk".to_owned(),
        })),
    ));
    frames.push(event(
        2000,
        6,
        1,
        span_end(900, End::InstallEventInfo(Default::default())),
    ));
    frames.push(event(
        2050,
        0,
        0,
        instant(Instant::InstallFinished(buck2_data::InstallFinished {
            duration: Some(dur(3)),
            ..Default::default()
        })),
    ));
    let entries = [
        Entry::GenericEntry(buck2_data::critical_path_entry2::GenericEntry {
            kind: "startup".to_owned(),
        }),
        Entry::Analysis(Default::default()),
        Entry::ActionExecution(Default::default()),
        Entry::FinalMaterialization(Default::default()),
        Entry::Waiting(Default::default()),
    ];
    frames.push(event(
        2051,
        0,
        0,
        instant(Instant::BuildGraphInfo(
            buck2_data::BuildGraphExecutionInfo {
                backend_name: Some("longest-path-graph".to_owned()),
                critical_path2: entries
                    .into_iter()
                    .enumerate()
                    .map(|(i, entry)| buck2_data::CriticalPathEntry2 {
                        entry: Some(entry),
                        span_ids: if i == 2 { vec![10] } else { vec![] },
                        duration: Some(dur(if i == 4 { 0 } else { 25 })),
                        start_offset_ns: Some(i as u64 * 25_000),
                        was_reused: Some(i == 4),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
        )),
    ));
    frames.push(event(
        2060,
        0,
        0,
        instant(Instant::Snapshot(Box::new(buck2_data::Snapshot {
            buck2_rss: Some(1100),
            buck2_max_rss: 9000,
            malloc_bytes_active: Some(600),
            buck2_user_cpu_us: 30,
            buck2_system_cpu_us: 5,
            re_download_bytes: 200,
            ..Default::default()
        }))),
    ));
    frames.push(event(
        2100,
        1,
        0,
        span_end(
            2000,
            End::Command(buck2_data::CommandEnd {
                // Retired scalar defaults false even on a successful install.
                is_success: false,
                data: Some(buck2_data::command_end::Data::Install(
                    buck2_data::InstallCommandEnd {
                        unresolved_target_patterns: vec![buck2_data::TargetPattern {
                            value: "fb4a".to_owned(),
                        }],
                    },
                )),
                ..Default::default()
            }),
        ),
    ));
    frames.push(frame(buck2_cli_proto::command_progress::Progress::Result(
        Box::new(buck2_cli_proto::CommandResult {
            result: Some(buck2_cli_proto::command_result::Result::InstallResponse(
                Default::default(),
            )),
            ..Default::default()
        }),
    )));
    let path = dir.join("command_profile_events.pb");
    std::fs::write(&path, frames.concat()).expect("test fixture dir must be writable");
    path
}
