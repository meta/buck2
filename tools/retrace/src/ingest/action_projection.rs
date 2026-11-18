/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Test-only reference projection of a paired action start/end.
//!
//! Preserve the original borrowed field selection independently of the
//! production normalizer, so naming, missingness, hashes and command details
//! can be compared without merely testing the same implementation twice.
//!
//! Field selection, missingness, naming, hashing and cloning intentionally
//! match scan::project_action. This module is not linked into production.

use super::scan::ActionRec;
use super::scan::CmdRec;
use super::scan::LastCmd;
use super::scan::dur_us;
use super::scan::key_hash;
use super::scan::remote_cache_origin;
use super::scan::ts_us;

pub(super) struct ProjectedAction {
    /// `cfg` and `category` are placeholders, NOT valid interner assignments.
    /// The ordered reducer must replace both before publishing this record.
    pub record: ActionRec,
    pub cfg: String,
    pub category: String,
}

/// Project only a pair already accepted by the ordered reducer. Orphan counts,
/// capture detection and first-local-host selection remain reducer operations
/// and must happen even for ends which have no paired start.
pub(super) fn project_action(
    sid: u64,
    ts: i64,
    dur: i64,
    start: buck2_data::ActionExecutionStart,
    end: Box<buck2_data::ActionExecutionEnd>,
) -> ProjectedAction {
    let (full_target, short_at, cfg) = names_of(&start);
    let category = start
        .name
        .as_ref()
        .map(|n| n.category.clone())
        .unwrap_or_default();

    let cmds: Vec<CmdRec> = end
        .commands
        .iter()
        .map(|c| {
            let md = c.details.as_ref().and_then(|d| d.metadata.as_ref());
            CmdRec {
                status: status_name(c),
                wall_time_us: md.and_then(|m| m.wall_time.as_ref()).map(dur_us),
                start_us: md.and_then(|m| m.start_time.as_ref()).map(ts_us),
                client_start_us: md
                    .and_then(|m| m.client_attempt.as_ref())
                    .and_then(|a| a.start_time.as_ref())
                    .map(ts_us),
                elastic: c
                    .details
                    .as_ref()
                    .and_then(super::scan::re_worker)
                    .is_some_and(|w| w.elastic_capacity),
            }
        })
        .collect();

    let mut last = LastCmd::default();
    if let Some(d) = end.commands.last().and_then(|c| c.details.as_ref()) {
        let md = d.metadata.as_ref();
        let stats = md.and_then(|m| m.execution_stats.as_ref());
        last.signed_exit_code = d.signed_exit_code;
        last.exec_us = md.and_then(|m| m.execution_time.as_ref()).map(dur_us);
        last.queue_us = md.and_then(|m| m.queue_duration.as_ref()).map(dur_us);
        last.input_fetch_us = md
            .and_then(|m| m.input_materialization_duration.as_ref())
            .map(dur_us);
        let worker = super::scan::re_worker(d);
        last.upload_us = worker
            .and_then(|w| w.output_upload_us)
            .and_then(super::scan::us_i64);
        last.worker_wall_us = worker
            .and_then(|w| w.worker_wall_time_us)
            .and_then(super::scan::us_i64);
        last.worker_bytes_in = worker.and_then(|w| w.input_download_bytes);
        last.worker_bytes_out = worker.and_then(|w| w.output_upload_bytes);
        let attempts = worker.and_then(|w| w.attempts.as_ref());
        last.attempts = attempts.map(|a| a.count).filter(|n| *n > 1);
        last.final_queue_us = attempts.and_then(|a| a.final_queue_us);
        last.hashing_us = md.and_then(|m| m.hashing_duration.as_ref()).map(dur_us);
        last.mem_peak = stats.and_then(|s| s.memory_peak);
        last.cpu_us = worker.and_then(super::scan::cpu_total_us);
        last.cpu_user_us = worker.and_then(|w| w.cpu_time_user_us);
        last.cpu_sys_us = worker.and_then(|w| w.cpu_time_system_us);
        last.elastic = worker.is_some_and(|w| w.elastic_capacity);
        if let Some(kind) = d.command_kind.as_ref() {
            use buck2_data::command_execution_kind::Command;
            last.hashing_local_outputs = matches!(
                &kind.command,
                Some(
                    Command::LocalCommand(_)
                        | Command::OmittedLocalCommand(_)
                        | Command::WorkerCommand(_)
                )
            );
            match &kind.command {
                Some(Command::LocalCommand(c)) => {
                    last.local_digest =
                        (!c.action_digest.is_empty()).then(|| c.action_digest.clone());
                    if !c.argv.is_empty() {
                        last.argv = Some(c.argv.clone());
                    }
                    last.cwd = c.context.as_ref().and_then(|x| x.working_directory.clone());
                    last.env_rule = c.context.as_ref().map_or(0, |x| x.environment_rule);
                }
                Some(Command::WorkerCommand(c)) if !c.argv.is_empty() => {
                    last.argv = Some(c.argv.clone());
                }
                Some(Command::WorkerInitCommand(c)) if !c.argv.is_empty() => {
                    last.argv = Some(c.argv.clone());
                }
                Some(Command::RemoteCommand(rc)) => {
                    last.remote = true;
                    // Enum zero alone does not establish reuse in old logs.
                    last.reused = rc.cache_hit || remote_cache_origin(rc.cache_hit_type);
                    last.remote_queue_us = rc.queue_time.as_ref().map(dur_us);
                    last.argv = Some(vec!["<remote>".to_owned(), rc.action_digest.clone()]);
                }
                Some(Command::OmittedLocalCommand(c)) => {
                    last.local_digest =
                        (!c.action_digest.is_empty()).then(|| c.action_digest.clone());
                }
                _ => {}
            }
        }
    }

    let digest = end.commands.iter().find_map(|c| {
        use buck2_data::command_execution_kind::Command;
        let kind = c.details.as_ref().and_then(|d| d.command_kind.as_ref())?;
        let d = match &kind.command {
            Some(Command::LocalCommand(x)) => &x.action_digest,
            Some(Command::RemoteCommand(x)) => &x.action_digest,
            Some(Command::WorkerCommand(x)) => &x.action_digest,
            Some(Command::OmittedLocalCommand(x)) => &x.action_digest,
            _ => return None,
        };
        (!d.is_empty()).then(|| d.clone())
    });

    let no_producers = buck2_data::ActionGraphProducers::default();
    let producers = end.graph_producers.as_ref().unwrap_or(&no_producers);
    ProjectedAction {
        record: ActionRec {
            sid,
            ts,
            dur,
            full_target,
            short_at,
            cfg: 0,
            category: 0,
            key_hash: start.key.as_ref().map(key_hash),
            kind: start.kind,
            failed: end.failed,
            error_kind: super::scan::action_error_kind(&end),
            execution_kind: end.execution_kind,
            cache_upload_result: end.cache_upload_result,
            dep_file_cache_upload_result: end.dep_file_cache_upload_result,
            wall_time_us: end.wall_time.as_ref().map(dur_us),
            prepare_us: end
                .run_action_timings
                .as_ref()
                .and_then(|t| t.prepare_command_us),
            finish_us: end
                .run_action_timings
                .as_ref()
                .and_then(|t| t.finish_action_us),
            output_size: end.output_size,
            input_files_bytes: end.input_files_bytes,
            invalidated: end.invalidation_info.is_some(),
            cmds,
            last,
            digest,
            execution: None,
            dep_span_ids: producers.span_ids.clone(),
            dep_action_keys: producers
                .reused_ids
                .iter()
                .map(|&id| u128::from(id))
                .collect(),
            dep_tset_ids: producers.tset_ids.clone(),
        },
        cfg,
        category,
    }
}

fn names_of(start: &buck2_data::ActionExecutionStart) -> (String, u32, String) {
    let what = match &start.name {
        Some(n) => format!("{} {}", n.category, n.identifier).trim().to_owned(),
        None => String::new(),
    };
    let (pkg, tname, cfg) = target_of(start);
    let short_at = u32::try_from(pkg.len() + 1).expect("package paths are far below 4 GiB");
    (format!("{pkg}:{tname} ({what})"), short_at, cfg)
}

fn target_of(start: &buck2_data::ActionExecutionStart) -> (String, String, String) {
    use buck2_data::action_key::Owner;
    let owner = start.key.as_ref().and_then(|k| k.owner.as_ref());
    match owner {
        Some(Owner::TargetLabel(c)) => {
            let (pkg, name) = match &c.label {
                Some(l) => (l.package.clone(), l.name.clone()),
                None => ("?".to_owned(), "?".to_owned()),
            };
            let cfg = c
                .configuration
                .as_ref()
                .map(|x| x.full_name.clone())
                .unwrap_or_default();
            (pkg, name, cfg)
        }
        Some(Owner::BxlKey(..)) => ("?".to_owned(), "BxlKey".to_owned(), String::new()),
        Some(Owner::TestTargetLabel(..)) => {
            ("?".to_owned(), "TestTargetLabel".to_owned(), String::new())
        }
        Some(Owner::AnonTarget(..)) => ("?".to_owned(), "AnonTarget".to_owned(), String::new()),
        Some(Owner::LocalResourceSetup(..)) => (
            "?".to_owned(),
            "LocalResourceSetup".to_owned(),
            String::new(),
        ),
        None => ("?".to_owned(), "?".to_owned(), String::new()),
    }
}

fn status_name(c: &buck2_data::CommandExecution) -> &'static str {
    use buck2_data::command_execution::Status;
    match &c.status {
        Some(Status::Success(..)) => "Success",
        Some(Status::Failure(..)) => "Failure",
        Some(Status::Timeout(..)) => "Timeout",
        Some(Status::Error(..)) => "Error",
        Some(Status::Cancelled(..)) => "Cancelled",
        Some(Status::WorkerFailure(..)) => "WorkerFailure",
        None => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    use prost::Message as _;

    use super::*;
    use crate::ingest::Staged;
    use crate::ingest::emit;
    use crate::ingest::infer;
    use crate::ingest::scan;
    use crate::ingest::testlog;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    // The fixture is independently decoded here, not rebuilt from the Scan's
    // already-projected fields. Pairing remains sequential like the reducer.
    fn project_fixture(path: &Path) -> Vec<ProjectedAction> {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;

        let input = std::fs::read(path).unwrap();
        let mut input = input.as_slice();
        buck2_data::Invocation::decode_length_delimited(&mut input).unwrap();
        let mut starts = HashMap::new();
        let mut projected = Vec::new();
        while !input.is_empty() {
            let frame =
                buck2_cli_proto::CommandProgress::decode_length_delimited(&mut input).unwrap();
            let Some(Progress::Event(event)) = frame.progress else {
                continue;
            };
            let Some(timestamp) = event.timestamp.as_ref().map(ts_us) else {
                continue;
            };
            match event.data {
                Some(Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::ActionExecution(start)),
                })) => {
                    starts.insert(event.span_id, (timestamp, start));
                }
                Some(Data::SpanEnd(end)) => {
                    let duration = end.duration.as_ref().map(dur_us).unwrap_or(0);
                    if let Some(buck2_data::span_end_event::Data::ActionExecution(end)) = end.data
                        && let Some((ts, start)) = starts.remove(&event.span_id)
                    {
                        projected.push(project_action(event.span_id, ts, duration, start, end));
                    }
                }
                _ => {}
            }
        }
        projected
    }

    fn emit_pair(scan: scan::Scan, dir: &Path, label: &str) -> (Vec<u8>, Option<Vec<u8>>) {
        let trace = Staged::beside(dir.join(format!("{label}-trace.json")));
        let graph = Staged::beside(dir.join(format!("{label}-graph.json")));
        let summary = emit::emit(scan, &trace, Some(&graph), false).unwrap();
        let graph_bytes = match summary.graph {
            emit::GraphOutcome::Written => Some(std::fs::read(&graph.write_to).unwrap()),
            emit::GraphOutcome::NotWritten => None,
        };
        (std::fs::read(&trace.write_to).unwrap(), graph_bytes)
    }

    fn assert_fixture_parity(make_log: impl FnOnce(&Path) -> std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "retrace-action-projection-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = make_log(&dir);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let reference = runtime.block_on(scan::scan(&infer(&log).unwrap())).unwrap();
        let mut actual = runtime.block_on(scan::scan(&infer(&log).unwrap())).unwrap();
        let projected = project_fixture(&log);
        assert_eq!(projected.len(), reference.actions.len());
        actual.actions = projected
            .into_iter()
            .zip(&reference.actions)
            .map(|(mut projected, expected)| {
                assert_eq!(projected.record.sid, expected.sid);
                assert_eq!(projected.cfg, reference.cfg_names[expected.cfg as usize]);
                assert_eq!(
                    projected.category,
                    reference.category_names[expected.category as usize]
                );
                assert_eq!(projected.record.dep_span_ids, expected.dep_span_ids);
                assert_eq!(projected.record.dep_action_keys, expected.dep_action_keys);
                assert_eq!(projected.record.dep_tset_ids, expected.dep_tset_ids);
                projected.record.cfg = expected.cfg;
                projected.record.category = expected.category;
                projected.record
            })
            .collect();
        assert_eq!(
            emit_pair(actual, &dir, "worker"),
            emit_pair(reference, &dir, "reference")
        );
    }

    #[test]
    fn owned_projection_matches_serial_projection_and_artifact_bytes() {
        assert_fixture_parity(|d| testlog::demo_log(d, Some(1), true, true));
        assert_fixture_parity(testlog::cache_hit_log);
        for (kind, hit) in [
            (2, false),
            (2, true),
            (3, false),
            (7, false),
            (9, false),
            (10, false),
        ] {
            for local_attempt in [false, true] {
                assert_fixture_parity(|d| {
                    testlog::cached_execution_log(d, kind, hit, local_attempt)
                });
            }
        }
    }

    #[test]
    fn upload_outcomes_match_reference_artifact_bytes() {
        for (cache, dep_file) in [(0, 0), (6, 1), (i32::MAX, -1)] {
            assert_fixture_parity(|dir| {
                let path = testlog::demo_log(dir, Some(1), true, true);
                testlog::rewrite(&path, |records| {
                    for record in records {
                        let Some(buck2_cli_proto::command_progress::Progress::Event(event)) =
                            &mut record.progress
                        else {
                            continue;
                        };
                        let Some(buck2_data::buck_event::Data::SpanEnd(end)) = &mut event.data
                        else {
                            continue;
                        };
                        if let Some(buck2_data::span_end_event::Data::ActionExecution(action)) =
                            &mut end.data
                        {
                            action.cache_upload_result = cache;
                            action.dep_file_cache_upload_result = dep_file;
                            action.allows_cache_upload = false;
                        }
                    }
                });
                path
            });
        }
    }

    #[test]
    fn names_and_owner_fallbacks_are_preserved_before_interning() {
        use buck2_data::action_key::Owner;

        let start = buck2_data::ActionExecutionStart {
            key: Some(buck2_data::ActionKey {
                owner: Some(Owner::TargetLabel(buck2_data::ConfiguredTargetLabel {
                    label: Some(buck2_data::TargetLabel {
                        package: "fbcode//démo".to_owned(),
                        name: "target".to_owned(),
                    }),
                    configuration: Some(buck2_data::Configuration {
                        full_name: "cfg:x".to_owned(),
                    }),
                    ..Default::default()
                })),
                ..Default::default()
            }),
            name: Some(buck2_data::ActionName {
                category: " rustc ".to_owned(),
                identifier: " lib.rs ".to_owned(),
            }),
            ..Default::default()
        };
        let p = project_action(17, 10, 20, start, Box::default());
        assert_eq!(p.record.full_target, "fbcode//démo:target (rustc   lib.rs)");
        assert_eq!(p.record.short(), "target (rustc   lib.rs)");
        assert_eq!(p.cfg, "cfg:x");
        assert_eq!(p.category, " rustc ");
        assert_eq!((p.record.cfg, p.record.category), (0, 0));
        for (owner, label) in [
            (Some(Owner::BxlKey(Default::default())), "BxlKey"),
            (
                Some(Owner::TestTargetLabel(Default::default())),
                "TestTargetLabel",
            ),
            (Some(Owner::AnonTarget(Default::default())), "AnonTarget"),
            (
                Some(Owner::LocalResourceSetup(Default::default())),
                "LocalResourceSetup",
            ),
            (None, "?"),
        ] {
            let start = buck2_data::ActionExecutionStart {
                key: Some(buck2_data::ActionKey {
                    owner,
                    ..Default::default()
                }),
                ..Default::default()
            };
            let p = project_action(17, 10, 20, start, Box::default());
            assert_eq!(p.record.full_target, format!("?:{label} ()"));
            assert!(p.cfg.is_empty() && p.category.is_empty());
        }
    }
}
