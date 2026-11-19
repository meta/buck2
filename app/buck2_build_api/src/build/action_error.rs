/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Schema for the structured action error within the build report.

use buck2_data::ActionError;
use buck2_data::CommandExecutionDetails;
use buck2_data::command_execution_kind::Command;
use buck2_event_observer::display::TargetDisplayOptions;
use buck2_event_observer::display::display_action_owner;
use buck2_event_observer::display::get_action_error_reason;
use serde::Serialize;

use crate::build::build_report::BuildReportCollector;

/// Maximum size for error content when truncation is enabled (20KB).
/// This matches the MAX_STRING_BYTES limit used in smart_truncate_event.rs for Scribe logging.
pub(crate) const MAX_ERROR_CONTENT_BYTES: usize = 20 * 1024;

/// Options for building action errors in build reports.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ActionErrorBuildOptions {
    /// Exclude error diagnostics from action errors.
    pub exclude_action_error_diagnostics: bool,
    /// Truncate error content to reduce build report size.
    pub truncate_error_content: bool,
}

#[derive(Debug, Clone, Serialize, PartialOrd, Ord, PartialEq, Eq)]
struct BuildReportActionName {
    category: String,
    identifier: String,
}

#[derive(Debug, Clone, Serialize, PartialOrd, Ord, PartialEq, Eq)]
struct BuildReportActionKey {
    owner: String,
}

#[derive(Debug, Clone, Serialize, PartialOrd, Ord, PartialEq, Eq)]
enum BuildReportActionErrorDiagnostics {
    #[serde(rename = "sub_errors")]
    SubErrors(Vec<BuildReportActionSubError>),
    #[serde(rename = "handler_invocation_error")]
    HandlerInvocationError(String),
}

#[derive(Debug, Clone, Serialize, PartialOrd, Ord, PartialEq, Eq)]
struct BuildReportActionSubError {
    category: String,
    message_content: Option<String>,
    // file path for the error location
    file: Option<String>,
    // Line number
    lnum: Option<u64>,
    // End line (for multi-line spans)
    end_lnum: Option<u64>,
    //  Column number
    col: Option<u64>,
    // End column (for ranges)
    end_col: Option<u64>,
    // Type of error (error, warning, info, etc.)
    error_type: Option<String>,
    // Numeric error code (e.g., 404, 500)
    error_number: Option<u64>,
    // Subcategory for finer-grained categorization
    subcategory: Option<String>,
    // Remediation steps for the error
    remediation: Option<String>,
}

/// DO NOT UPDATE WITHOUT UPDATING `docs/users/build_observability/build_report.md`!
#[derive(Debug, Clone, Serialize, PartialOrd, Ord, PartialEq, Eq)]
pub(crate) struct BuildReportActionError {
    name: BuildReportActionName,
    key: BuildReportActionKey,
    digest: String,
    error_content: String,
    stderr_content: String,
    stdout_content: String,
    error_diagnostics: Option<BuildReportActionErrorDiagnostics>,
}

impl BuildReportActionError {
    pub(crate) fn new<'a>(
        error: &ActionError,
        collector: &mut BuildReportCollector<'a>,
        opts: ActionErrorBuildOptions,
    ) -> Self {
        let reason = get_action_error_reason(error).ok().unwrap_or_default();

        let command_details = error.last_command.as_ref().and_then(|c| c.details.as_ref());

        let owner = error.key.as_ref().map_or(String::default(), |key| {
            key.owner.as_ref().map_or(String::default(), |owner| {
                display_action_owner(owner, TargetDisplayOptions::for_build_report())
                    .unwrap_or_default()
            })
        });

        let key = BuildReportActionKey { owner };

        let name = BuildReportActionName {
            category: error
                .name
                .as_ref()
                .map_or(String::default(), |name| name.category.clone()),
            identifier: error
                .name
                .as_ref()
                .map_or(String::default(), |name| name.identifier.clone()),
        };

        let error_diagnostics = if opts.exclude_action_error_diagnostics {
            None
        } else {
            error.error_diagnostics.clone().map(|error_diagnostics| {
                match error_diagnostics.data.unwrap() {
                    buck2_data::action_error_diagnostics::Data::SubErrors(sub_errors) => {
                        let sub_errors = sub_errors
                            .sub_errors
                            .iter()
                            .map(|s| BuildReportActionSubError {
                                category: s.category.clone(),
                                message_content: s
                                    .message
                                    .clone()
                                    .map(|m| collector.update_string_cache(m)),
                                file: s.file.clone(),
                                lnum: s.lnum,
                                end_lnum: s.end_lnum,
                                col: s.col,
                                end_col: s.end_col,
                                error_type: s.error_type.clone(),
                                error_number: s.error_number,
                                subcategory: s.subcategory.clone(),
                                remediation: s.remediation.clone(),
                            })
                            .collect();
                        BuildReportActionErrorDiagnostics::SubErrors(sub_errors)
                    }
                    buck2_data::action_error_diagnostics::Data::HandlerInvocationError(
                        invocation_failure,
                    ) => BuildReportActionErrorDiagnostics::HandlerInvocationError(
                        collector.update_string_cache(invocation_failure.clone()),
                    ),
                }
            })
        };

        let stderr = command_details.map_or(String::default(), |c| {
            console::strip_ansi_codes(&c.cmd_stderr).to_string()
        });
        let stdout = command_details.map_or(String::default(), |c| c.cmd_stdout.clone());

        // Apply truncation if enabled
        let (reason, stderr, stdout) = if opts.truncate_error_content {
            (
                buck2_util::truncate::truncate(&reason, MAX_ERROR_CONTENT_BYTES),
                buck2_util::truncate::truncate(&stderr, MAX_ERROR_CONTENT_BYTES),
                buck2_util::truncate::truncate(&stdout, MAX_ERROR_CONTENT_BYTES),
            )
        } else {
            (reason, stderr, stdout)
        };

        let error_content = collector.update_string_cache(reason);
        let stderr_content = collector.update_string_cache(stderr);
        let stdout_content = collector.update_string_cache(stdout);

        Self {
            key,
            name,
            error_content,
            stderr_content,
            stdout_content,
            digest: get_action_digest(command_details).unwrap_or_default(),
            error_diagnostics,
        }
    }
}

fn get_action_digest(command_details: Option<&CommandExecutionDetails>) -> Option<String> {
    command_details.and_then(|command_details| {
        if let Some(command_kind) = &command_details.command_kind {
            match command_kind.command.as_ref() {
                Some(Command::RemoteCommand(remote_command)) => {
                    Some(remote_command.action_digest.to_owned())
                }
                Some(Command::LocalCommand(local_command)) => {
                    Some(local_command.action_digest.to_owned())
                }
                Some(Command::WorkerCommand(worker_command)) => {
                    Some(worker_command.action_digest.to_owned())
                }
                Some(Command::OmittedLocalCommand(omitted_local_command)) => {
                    Some(omitted_local_command.action_digest.to_owned())
                }
                _ => None,
            }
        } else {
            None
        }
    })
}
