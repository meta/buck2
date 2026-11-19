/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

fn truncate(s: &str, max_bytes: usize) -> String {
    // Scuba has a tendency to OOM on queries if we ever report very long strings, so truncate them
    // to a reasonable length unconditionally
    const MAX_STRING_BYTES: usize = 20 * 1024;

    buck2_util::truncate::truncate(s, max_bytes.min(MAX_STRING_BYTES))
}

#[cfg_attr(not(fbcode_build), allow(dead_code))]
pub(crate) fn smart_truncate_event(d: &mut buck2_data::buck_event::Data) {
    use buck2_data::buck_event::Data;

    match d {
        Data::SpanEnd(s) => {
            use buck2_data::span_end_event::Data;

            match &mut s.data {
                Some(Data::ActionExecution(action_execution)) => {
                    truncate_action_execution_end(action_execution);
                }
                Some(Data::Command(command_end)) => {
                    truncate_command_end(command_end, false);
                }
                Some(Data::TestRun(test_end)) => {
                    truncate_test_end(test_end);
                }
                Some(Data::TestDiscovery(test_discovery_end)) => {
                    truncate_test_discovery_end(test_discovery_end);
                }
                _ => {}
            };
        }
        Data::Instant(inst) => {
            use buck2_data::instant_event::Data;
            if let Some(Data::TargetPatterns(target_patterns)) = &mut inst.data {
                truncate_target_patterns(&mut target_patterns.target_patterns);
            }
        }
        Data::Record(rec) => {
            if let Some(buck2_data::record_event::Data::InvocationRecord(invocation_record)) =
                &mut rec.data
            {
                truncate_invocation_record(invocation_record)
            }
        }
        _ => {}
    };
}

fn truncate_invocation_record(invocation_record: &mut buck2_data::InvocationRecord) {
    // FIXME(JakobDegen): The sum of the per-field limits adds up to more than the 1MB scribe limits
    if let Some(ref mut file_watcher_stats) = invocation_record.file_watcher_stats {
        truncate_file_watcher_stats(file_watcher_stats);
    }
    if let Some(ref mut resolved_target_patterns) = invocation_record.parsed_target_patterns {
        truncate_target_patterns(&mut resolved_target_patterns.target_patterns);
        // Clear `unresolved_target_patterns` to save bandwidth. It has less information
        // than `resolved` one does, and will never be used if `resolved` one is available.
        if let Some(ref mut command_end) = invocation_record.command_end {
            truncate_command_end(command_end, true);
        }
    } else if let Some(ref mut command_end) = invocation_record.command_end {
        truncate_command_end(command_end, false);
    }

    const MAX_CLI_ARGS_BYTES: usize = 512 * 1024;
    let orig_len = invocation_record.cli_args.len();
    let mut bytes: usize = 0;
    for (index, arg) in invocation_record.cli_args.iter().enumerate() {
        bytes += arg.len();
        if bytes > MAX_CLI_ARGS_BYTES {
            invocation_record.cli_args.truncate(index);
            invocation_record
                .cli_args
                .push(format!("<<Truncated (reported {index} / {orig_len})>>"));
            break;
        }
    }

    const MAX_ERROR_REPORT_BYTES: usize = 512 * 1024;
    let max_per_report = MAX_ERROR_REPORT_BYTES / invocation_record.errors.len().max(1);
    for error in &mut invocation_record.errors {
        error.message = truncate(&error.message, max_per_report / 2);
        if let Some(telemetry_message) = &mut error.telemetry_message {
            *telemetry_message = truncate(telemetry_message, max_per_report / 2);
        }
    }
}

fn truncate_action_execution_end(action_execution_end: &mut buck2_data::ActionExecutionEnd) {
    let per_command_size_budget = (500 * 1024) / action_execution_end.commands.len().max(1);

    let truncate_cmd = |cmd: &mut buck2_data::CommandExecution, truncate_all: bool| {
        if let Some(details) = &mut cmd.details {
            details.cmd_stderr = if truncate_all {
                "<<omitted>>".to_owned()
            } else {
                truncate(
                    &console::strip_ansi_codes(&details.cmd_stderr),
                    per_command_size_budget,
                )
            };
        }
    };

    if let Some((last_command, retries)) = action_execution_end.commands.split_last_mut() {
        for retried in retries {
            truncate_cmd(retried, false);
        }
        // Current Scribe tailers don't read stderr of successful actions.
        // Save some bytes.
        truncate_cmd(last_command, !action_execution_end.failed);
    }
}

fn truncate_command_end(command_end: &mut buck2_data::CommandEnd, clear_target_patterns: bool) {
    use buck2_data::command_end::Data;

    if let Some(ref mut target_patterns) = match &mut command_end.data {
        Some(Data::Build(build_command_end)) => {
            Some(&mut build_command_end.unresolved_target_patterns)
        }
        Some(Data::Test(test_command_end)) => {
            Some(&mut test_command_end.unresolved_target_patterns)
        }
        Some(Data::Install(install_command_end)) => {
            Some(&mut install_command_end.unresolved_target_patterns)
        }
        Some(Data::Targets(targets_command_end)) => {
            Some(&mut targets_command_end.unresolved_target_patterns)
        }
        _ => None,
    } {
        if clear_target_patterns {
            target_patterns.clear();
        } else {
            truncate_target_patterns(target_patterns);
        }
    }
}

fn truncate_file_watcher_stats(file_watcher_stats: &mut buck2_data::FileWatcherStats) {
    const MAX_FILE_CHANGE_BYTES: usize = 100 * 1024;
    let mut bytes: usize = 0;
    for (index, ev) in file_watcher_stats.events.iter().enumerate() {
        bytes += ev.path.len();
        if bytes > MAX_FILE_CHANGE_BYTES {
            file_watcher_stats.events.truncate(index);
            file_watcher_stats.incomplete_events_reason = Some(format!(
                "Too long file change records ({bytes} bytes, max {MAX_FILE_CHANGE_BYTES} bytes)"
            ));
            break;
        }
    }
}

fn truncate_test_end(test_end: &mut buck2_data::TestRunEnd) {
    const MAX_TEST_NAMES_BYTES: usize = 512 * 1024;
    if let Some(ref mut suite) = test_end.suite {
        let orig_len = suite.test_names.len();
        let mut bytes: usize = 0;
        for (index, test_name) in suite.test_names.iter().enumerate() {
            bytes += test_name.len();
            if bytes > MAX_TEST_NAMES_BYTES {
                suite.test_names.truncate(index);
                let warn = format!("<<Truncated (reported {index} / {orig_len})>>");
                suite.test_names.push(warn);
                break;
            }
        }
    }

    // Scribe tailer logs neither stdout nor stderr of tests, so don't send these.
    if let Some(ref mut command_report) = test_end.command_report {
        if let Some(ref mut details) = command_report.details {
            if !details.cmd_stdout.is_empty() {
                details.cmd_stdout = "<<omitted>>".to_owned();
            }
            if !details.cmd_stderr.is_empty() {
                details.cmd_stderr = "<<omitted>>".to_owned();
            }
        }
    }
}

fn truncate_test_discovery_end(test_discovery_end: &mut buck2_data::TestDiscoveryEnd) {
    // Scribe tailer logs neither stdout nor stderr of test discovery, so don't send these.
    if let Some(ref mut command_report) = test_discovery_end.command_report {
        if let Some(ref mut details) = command_report.details {
            if !details.cmd_stdout.is_empty() {
                details.cmd_stdout = "<<omitted>>".to_owned();
            }
            if !details.cmd_stderr.is_empty() {
                details.cmd_stderr = "<<omitted>>".to_owned();
            }
        }
    }
}

fn truncate_target_patterns(target_patterns: &mut Vec<buck2_data::TargetPattern>) {
    const MAX_TARGET_PATTERNS_BYTES: usize = 512 * 1024;
    let orig_len = target_patterns.len();
    let mut bytes: usize = 0;
    for (index, target) in target_patterns.iter().enumerate() {
        bytes += target.value.len();
        if bytes > MAX_TARGET_PATTERNS_BYTES {
            target_patterns.truncate(index);
            let warn = format!("<<Truncated (reported {index} / {orig_len})>>");
            target_patterns.push(buck2_data::TargetPattern { value: warn });
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sink::smart_truncate_event::smart_truncate_event;

    fn make_invocation_record(data: buck2_data::InvocationRecord) -> buck2_data::buck_event::Data {
        buck2_data::buck_event::Data::Record(buck2_data::RecordEvent {
            data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                data,
            ))),
        })
    }

    fn make_action_execution_end(
        data: buck2_data::ActionExecutionEnd,
    ) -> buck2_data::buck_event::Data {
        buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
            data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                data,
            ))),
            ..Default::default()
        })
    }

    fn make_command_end(data: buck2_data::CommandEnd) -> buck2_data::buck_event::Data {
        buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
            data: Some(buck2_data::span_end_event::Data::Command(data)),
            ..Default::default()
        })
    }

    fn make_build_command_end(
        unresolved_target_patterns: Vec<buck2_data::TargetPattern>,
    ) -> buck2_data::CommandEnd {
        buck2_data::CommandEnd {
            data: Some(buck2_data::command_end::Data::Build(
                buck2_data::BuildCommandEnd {
                    unresolved_target_patterns,
                },
            )),
            ..Default::default()
        }
    }

    fn make_test_end(data: buck2_data::TestRunEnd) -> buck2_data::buck_event::Data {
        buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
            data: Some(buck2_data::span_end_event::Data::TestRun(Box::new(data))),
            ..Default::default()
        })
    }

    fn make_test_discovery_end(data: buck2_data::TestDiscoveryEnd) -> buck2_data::buck_event::Data {
        buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
            data: Some(buck2_data::span_end_event::Data::TestDiscovery(Box::new(
                data,
            ))),
            ..Default::default()
        })
    }

    fn make_command_execution_with_stderr(stderr: String) -> buck2_data::CommandExecution {
        buck2_data::CommandExecution {
            details: Some(buck2_data::CommandExecutionDetails {
                cmd_stderr: stderr,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn smart_truncate_resolved_target_patterns_clears_unresolved_one() {
        let mut record = buck2_data::InvocationRecord::default();
        let mut record_expected = record.clone();

        let resolved_target_patterns = vec![buck2_data::TargetPattern {
            value: "some_resolved_target".to_owned(),
        }];
        record.parsed_target_patterns = Some(buck2_data::ParsedTargetPatterns {
            target_patterns: resolved_target_patterns.clone(),
        });
        // resolved_target_patterns is expected to be unchanged.
        record_expected.parsed_target_patterns = Some(buck2_data::ParsedTargetPatterns {
            target_patterns: resolved_target_patterns,
        });

        let unresolved_target_patterns = vec![buck2_data::TargetPattern {
            value: "some_unresolved_target".to_owned(),
        }];
        record.command_end = Some(make_build_command_end(unresolved_target_patterns));

        // unresolved_target_patterns is expected to be empty.
        record_expected.command_end = Some(make_build_command_end(vec![]));

        let mut event_data = make_invocation_record(record);
        let event_data_expected = make_invocation_record(record_expected);

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_unresolved_target_used_when_resolved_one_unavailable() {
        let mut record = buck2_data::InvocationRecord::default();
        let mut record_expected = record.clone();

        record.parsed_target_patterns = None;
        record_expected.parsed_target_patterns = None;

        let unresolved_target_patterns = vec![buck2_data::TargetPattern {
            value: "some_unresolved_target".to_owned(),
        }];
        let command_end = make_build_command_end(unresolved_target_patterns);

        record.command_end = Some(command_end.clone());
        // unresolved_target_patterns is expected to be unchanged.
        record_expected.command_end = Some(command_end);

        let mut event_data = make_invocation_record(record);
        let event_data_expected = make_invocation_record(record_expected);

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_action_execution_end_one_last_command_truncated() {
        let command_execution_with_stderr =
            make_command_execution_with_stderr("this is a test".to_owned());
        let command_execution_stderr_omitted =
            make_command_execution_with_stderr("<<omitted>>".to_owned());

        let action_execution_end_with_stderrs = buck2_data::ActionExecutionEnd {
            commands: vec![command_execution_with_stderr],
            ..Default::default()
        };
        let action_execution_end_last_stderr_omitted = buck2_data::ActionExecutionEnd {
            commands: vec![command_execution_stderr_omitted],
            ..Default::default()
        };
        let mut event_data = make_action_execution_end(action_execution_end_with_stderrs);
        let event_data_expected =
            make_action_execution_end(action_execution_end_last_stderr_omitted);

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_action_execution_end_long_stderr_command_truncated() {
        let command_execution_with_stderr =
            make_command_execution_with_stderr("this is a test".to_owned());
        let mut over_sized_str = "0123456789".repeat(2 * 1024);
        over_sized_str.push_str("0123456789"); // 20k + 10; 10-byte over
        let command_execution_with_long_stderr = make_command_execution_with_stderr(over_sized_str);
        let mut omitted_str = "0123456789".repeat(2 * 1024);
        omitted_str.replace_range((10 * 1024 - 6)..(10 * 1024 + 6), "<<omitted>>");
        let command_execution_stderr_partially_omitted =
            make_command_execution_with_stderr(omitted_str);
        let command_execution_stderr_all_omitted =
            make_command_execution_with_stderr("<<omitted>>".to_owned());

        let action_execution_end_with_stderrs = buck2_data::ActionExecutionEnd {
            commands: vec![
                command_execution_with_stderr.clone(),
                command_execution_with_long_stderr.clone(),
                command_execution_with_stderr.clone(),
                command_execution_with_long_stderr,
                command_execution_with_stderr.clone(),
            ],
            ..Default::default()
        };
        let action_execution_end_last_stderr_omitted = buck2_data::ActionExecutionEnd {
            commands: vec![
                command_execution_with_stderr.clone(),
                command_execution_stderr_partially_omitted.clone(),
                command_execution_with_stderr,
                command_execution_stderr_partially_omitted,
                command_execution_stderr_all_omitted,
            ],
            ..Default::default()
        };
        let mut event_data = make_action_execution_end(action_execution_end_with_stderrs);
        let event_data_expected =
            make_action_execution_end(action_execution_end_last_stderr_omitted);

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_build_command_end_short_target_patterns_not_truncated() {
        let unresolved_target_patterns = vec![
            buck2_data::TargetPattern {
                value: "hello".to_owned(),
            },
            buck2_data::TargetPattern {
                value: "world".to_owned(),
            },
            buck2_data::TargetPattern {
                value: "!\n".to_owned(),
            },
        ];
        let command_end = make_build_command_end(unresolved_target_patterns);

        let mut event_data = make_command_end(command_end);
        let event_data_expected = event_data.clone();

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_build_command_end_long_target_patterns_truncated() {
        let unresolved_target_patterns = vec![
            buck2_data::TargetPattern {
                value: "0123456789".repeat(20 * 1024),
            },
            buck2_data::TargetPattern {
                value: "0123456789".repeat(20 * 1024),
            },
            buck2_data::TargetPattern {
                value: "0123456789".repeat(20 * 1024), // 600k in total; 88k-byte over
            },
        ];
        let command_end = make_build_command_end(unresolved_target_patterns);

        let unresolved_target_patterns_truncated = vec![
            buck2_data::TargetPattern {
                value: "0123456789".repeat(20 * 1024),
            },
            buck2_data::TargetPattern {
                value: "0123456789".repeat(20 * 1024),
            },
            buck2_data::TargetPattern {
                value: "<<Truncated (reported 2 / 3)>>".to_owned(),
            },
        ];
        let command_end_truncated = make_build_command_end(unresolved_target_patterns_truncated);

        let mut event_data = make_command_end(command_end);
        let event_data_expected = make_command_end(command_end_truncated);

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_long_file_watcher_stats_truncated() {
        let file_watcher_event = buck2_data::FileWatcherEvent {
            path: "0123456789".repeat(3 * 1024),
            ..Default::default()
        };
        let file_watcher_stats = buck2_data::FileWatcherStats {
            events: vec![
                file_watcher_event.clone(),
                file_watcher_event.clone(),
                file_watcher_event.clone(),
                file_watcher_event.clone(), // 120k in total; 20k-byte over
            ],
            ..Default::default()
        };
        let file_watcher_stats_truncated = buck2_data::FileWatcherStats {
            events: vec![
                file_watcher_event.clone(),
                file_watcher_event.clone(),
                file_watcher_event,
            ],
            incomplete_events_reason: Some(format!(
                "Too long file change records ({} bytes, max {} bytes)",
                120 * 1024,
                100 * 1024
            )),
            ..Default::default()
        };
        let record = buck2_data::InvocationRecord {
            file_watcher_stats: Some(file_watcher_stats),
            ..Default::default()
        };
        let record_truncated = buck2_data::InvocationRecord {
            file_watcher_stats: Some(file_watcher_stats_truncated),
            ..Default::default()
        };
        let mut event_data = make_invocation_record(record);
        let event_data_expected = make_invocation_record(record_truncated);

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_short_file_watcher_stats_not_truncated() {
        let file_watcher_event = buck2_data::FileWatcherEvent {
            path: "this is a test".to_owned(),
            ..Default::default()
        };
        let file_watcher_stats = buck2_data::FileWatcherStats {
            events: vec![
                file_watcher_event.clone(),
                file_watcher_event.clone(),
                file_watcher_event,
            ],
            ..Default::default()
        };
        let record = buck2_data::InvocationRecord {
            file_watcher_stats: Some(file_watcher_stats),
            ..Default::default()
        };
        let mut event_data = make_invocation_record(record);
        let event_data_expected = event_data.clone();

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_invocation_record_long_cli_args_truncated() {
        let cli_args = vec![
            "0123456789".repeat(20 * 1024),
            "0123456789".repeat(20 * 1024),
            "0123456789".repeat(20 * 1024), // 600k in total; 88k-byte over
        ];
        let cli_args_truncated = vec![
            "0123456789".repeat(20 * 1024),
            "0123456789".repeat(20 * 1024),
            "<<Truncated (reported 2 / 3)>>".to_owned(),
        ];

        let record = buck2_data::InvocationRecord {
            cli_args,
            ..Default::default()
        };
        let record_truncated = buck2_data::InvocationRecord {
            cli_args: cli_args_truncated,
            ..Default::default()
        };

        let mut event_data = make_invocation_record(record);
        let event_data_expected = make_invocation_record(record_truncated);

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_invocation_record_short_cli_args_truncated() {
        let cli_args = vec!["this is".to_owned(), "a test".to_owned()];

        let record = buck2_data::InvocationRecord {
            cli_args,
            ..Default::default()
        };

        let mut event_data = make_invocation_record(record);
        let event_data_expected = event_data.clone();

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_invocation_record_error_reports_truncated() {
        let errors = vec![
            buck2_data::ProcessedErrorReport {
                message: "0123456789".repeat(200 * 1024),
                telemetry_message: None,
                ..Default::default()
            },
            buck2_data::ProcessedErrorReport {
                message: "0123456789".repeat(200 * 1024),
                telemetry_message: Some("0123456789".repeat(200 * 1024)),
                ..Default::default()
            },
        ];

        let mut event_data = make_invocation_record(buck2_data::InvocationRecord {
            errors,
            ..Default::default()
        });
        smart_truncate_event(&mut event_data);

        let buck2_data::buck_event::Data::Record(record_event) = event_data else {
            unreachable!()
        };
        let Some(buck2_data::record_event::Data::InvocationRecord(invocation_record)) =
            record_event.data
        else {
            unreachable!()
        };
        let size = invocation_record
            .errors
            .into_iter()
            .map(|e| e.message.len() + e.telemetry_message.map_or(0, |s| s.len()))
            .sum::<usize>();
        assert!(size < 500 * 1024);
    }

    #[test]
    fn smart_truncate_test_end_long_test_names_truncated() {
        let test_names = vec![
            "0123456789".repeat(20 * 1024),
            "0123456789".repeat(20 * 1024),
            "0123456789".repeat(20 * 1024), // 600k in total; 88k-byte over
        ];
        let test_names_truncated = vec![
            "0123456789".repeat(20 * 1024),
            "0123456789".repeat(20 * 1024),
            "<<Truncated (reported 2 / 3)>>".to_owned(),
        ];

        let test_end = buck2_data::TestRunEnd {
            suite: Some(buck2_data::TestSuite {
                test_names,
                ..Default::default()
            }),
            ..Default::default()
        };
        let test_end_truncated = buck2_data::TestRunEnd {
            suite: Some(buck2_data::TestSuite {
                test_names: test_names_truncated,
                ..Default::default()
            }),
            ..Default::default()
        };

        let mut event_data = make_test_end(test_end);
        let event_data_expected = make_test_end(test_end_truncated);

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    fn make_command_execution_with_stdout(stdout: String) -> buck2_data::CommandExecution {
        buck2_data::CommandExecution {
            details: Some(buck2_data::CommandExecutionDetails {
                cmd_stderr: stdout,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn smart_truncate_test_end_command_report_stdout_truncated() {
        let test_end = buck2_data::TestRunEnd {
            command_report: Some(make_command_execution_with_stdout("blah".to_owned())),
            ..Default::default()
        };

        let test_end_truncated = buck2_data::TestRunEnd {
            command_report: Some(make_command_execution_with_stdout("<<omitted>>".to_owned())),
            ..Default::default()
        };

        let mut event_data = make_test_end(test_end);
        let event_data_expected = make_test_end(test_end_truncated);

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }

    #[test]
    fn smart_truncate_test_discovery_end_command_report_stdout_truncated() {
        let test_discovery_end = buck2_data::TestDiscoveryEnd {
            command_report: Some(make_command_execution_with_stdout("blah".to_owned())),
            ..Default::default()
        };

        let test_discovery_end_truncated = buck2_data::TestDiscoveryEnd {
            command_report: Some(make_command_execution_with_stdout("<<omitted>>".to_owned())),
            ..Default::default()
        };

        let mut event_data = make_test_discovery_end(test_discovery_end);
        let event_data_expected = make_test_discovery_end(test_discovery_end_truncated);

        smart_truncate_event(&mut event_data);

        assert_eq!(event_data, event_data_expected);
    }
}
