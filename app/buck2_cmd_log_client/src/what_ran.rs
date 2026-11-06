/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::io::Write;

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ClientIoError;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_data::SchedulingMode;
use buck2_data::re_platform::Property;
use buck2_error::conversion::from_any_with_tag;
use buck2_event_log::stream_value::StreamValue;
use buck2_event_observer::fmt_duration;
use buck2_event_observer::what_ran;
use buck2_event_observer::what_ran::CommandReproducer;
use buck2_event_observer::what_ran::WhatRanOptions;
use buck2_event_observer::what_ran::WhatRanOutputCommand;
use buck2_event_observer::what_ran::WhatRanOutputCommandExtra;
use buck2_event_observer::what_ran::WhatRanOutputWriter;
use buck2_event_observer::what_ran::WhatRanRelevantAction;
use buck2_event_observer::what_ran::WhatRanState;
use buck2_events::span::SpanId;
use buck2_hash::BuckMutInsertionMap;
use futures::TryStreamExt;
use futures::stream::Stream;

use crate::LogCommandOutputFormat;
use crate::LogCommandOutputFormatWithWriter;
use crate::OutputFormatWithWriter;
use crate::transform_format;

/// Output everything that buck ran from the selected invocation. If no invocation was specified,
/// use the last buck invocation from this isolation directory.
///
/// The output is presented as a series of tab-delimited records with the following structure:
///
/// build    fbsource//your:target    local    clang foo.c
///
/// 1: The reason for executing a given command. That's either to build or to test.
///
/// 2: The identity of this command. This will include the target that required it.
///
/// 3: The executor for this command. This will either be RE or local.
///
/// 4: Details to reproduce it. For RE, that's the action digest. For local, the command.
///
/// To reproduce an action that ran on RE, use the following command then follow the instructions.
/// The DIGEST is of the form `hash:size`.
///
/// $ frecli cas download-action DIGEST
///
/// To reproduce an action that ran locally, make sure your working directory is the project root
/// (if unsure, use `buck2 root --kind project` to find it), then run the command. The command is
/// already shell-quoted.
#[derive(Debug, clap::Parser)]
pub struct WhatRanCommand {
    #[clap(flatten)]
    pub common: WhatRanCommandCommon,

    /// Show only commands that failed
    #[clap(long, conflicts_with = "incomplete")]
    pub failed: bool,

    /// Show only commands that were not completed.
    /// That is command were running if buck2 process was killed,
    /// or command currently running if buck2 is running build now.
    #[clap(long)]
    pub incomplete: bool,

    /// Show also std_err from commands that are run.
    /// If the command fails before completing, we display "<command did not finish executing>".
    /// If it finishes but there is no error, we display "<stderr is empty>".
    /// Otherwise, std_err is shown. For JSON, we show raw values and null for non-completion.
    #[clap(long, conflicts_with = "incomplete")]
    pub show_std_err: bool,

    /// Omit commands if their std_err is empty
    #[clap(long, conflicts_with = "incomplete", requires = "show_std_err")]
    pub omit_empty_std_err: bool,
}

#[derive(Debug, clap::Parser)]
pub struct WhatRanCommandCommon {
    #[clap(flatten)]
    event_log: EventLogOptions,

    #[clap(flatten)]
    output: LogCommandOutputFormat,

    #[clap(flatten)]
    options: WhatRanOptions,
}

struct WhatRanCommandOptions {
    options: WhatRanOptions,

    /// Print commands only if they failed.
    failed: bool,

    /// Print commands only if they did not finish.
    incomplete: bool,
}

impl BuckSubcommand for WhatRanCommand {
    const COMMAND_NAME: &'static str = "log-what-ran";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let Self {
            common:
                WhatRanCommandCommon {
                    event_log,
                    output,
                    options,
                },
            failed,
            incomplete,
            show_std_err,
            omit_empty_std_err,
        } = self;
        buck2_client_ctx::stdio::print_with_writer::<buck2_error::Error, _>(async move |w| {
            let mut output = OutputFormatWithWriter {
                format: transform_format(output, w),
                include_std_err: show_std_err,
                omit_empty_std_err,
            };
            let log_path = event_log.get(&ctx).await?;

            let (invocation, events) = log_path.unpack_stream().await?;

            buck2_client_ctx::eprintln!(
                "Showing commands from: {}{}",
                invocation.display_command_line(),
                if options.filter_category.is_some() {
                    ", filtered by action category"
                } else {
                    ""
                }
            )?;

            let options = WhatRanCommandOptions {
                options,
                failed,
                incomplete,
            };
            WhatRanCommandState::execute(events, &mut output, &options).await?;
            buck2_error::Ok(())
        })
        .await?;
        ExitResult::success()
    }
}

#[allow(clippy::vec_box)]
struct WhatRanEntry {
    action: WhatRanRelevantAction,
    reproducers: Vec<CommandReproducer>,
}

impl WhatRanEntry {
    fn emit_what_ran_entry(
        self,
        output: &mut impl WhatRanOutputWriter,
        options: &WhatRanCommandOptions,
        std_err: Option<&str>,
        duration: Option<std::time::Duration>,
        scheduling_mode: Option<SchedulingMode>,
    ) -> Result<(), ClientIoError> {
        let action = &self.action;
        let options_regex = what_ran::WhatRanOptionsRegex::from_options(&options.options)?;
        for repro in self.reproducers.into_iter() {
            what_ran::emit_what_ran_entry(
                Some(action),
                repro,
                output,
                &options_regex,
                std_err,
                duration,
                scheduling_mode,
            )?;
        }
        Ok(())
    }
}

/// The state for a WhatRan command. This is all the events we have seen that are
/// we have seen that are WhatRanRelevantActions, and the CommandReproducer associated with them.
#[derive(Default)]
pub struct WhatRanCommandState {
    /// Maps action spans to their details, in the order the spans started: unfinished spans are
    /// printed from this map at the end.
    known_actions: BuckMutInsertionMap<SpanId, WhatRanEntry>,
}

impl WhatRanState for WhatRanCommandState {
    fn get(&self, span_id: SpanId) -> Option<WhatRanRelevantAction> {
        self.known_actions.get(&span_id).map(|e| e.action.clone())
    }
}

impl WhatRanCommandState {
    async fn execute(
        mut events: impl Stream<Item = buck2_error::Result<StreamValue>> + Unpin + Send,
        output: &mut impl WhatRanOutputWriter,
        options: &WhatRanCommandOptions,
    ) -> Result<(), ClientIoError> {
        let mut cmd = Self::default();

        while let Some(event) = events.try_next().await? {
            if let StreamValue::Event(event) = event {
                cmd.event(event, output, options)?;
            }
        }

        // emit remaining
        cmd.emit_remaining(output, options)?;
        Ok(())
    }

    fn emit_remaining(
        self,
        output: &mut impl WhatRanOutputWriter,
        options: &WhatRanCommandOptions,
    ) -> buck2_error::Result<()> {
        for (_, entry) in self.known_actions.into_iter() {
            if should_emit_unfinished_action(options) {
                entry.emit_what_ran_entry(output, options, None, None, None)?;
            }
        }
        Ok(())
    }

    /// Receive a new event. We store it if it's relevant and emit them later.
    ///
    /// For each entry we emit we track 4 types of events.
    /// - action start, to create WhatRanRelevantAction
    /// - executor stage, to create CommandReproducer to add to WhatRanRelevantAction
    /// - action end, to emit WhatRanRelevantAction if action finished. If not, action is considered
    ///   unfinished. We check to emit all unfinished after all events are received
    fn event(
        &mut self,
        event: Box<buck2_data::BuckEvent>,
        output: &mut impl WhatRanOutputWriter,
        options: &WhatRanCommandOptions,
    ) -> buck2_error::Result<()> {
        if let Some(data) = event.data {
            // Create WhatRanRelevantAction on SpanStart to track CommandReproducers as they come
            if let Some(action) = WhatRanRelevantAction::from_buck_data(&data) {
                self.known_actions.insert(
                    SpanId::from_u64(event.span_id)?,
                    WhatRanEntry {
                        action,
                        reproducers: Default::default(),
                    },
                );
                return Ok(());
            }
            // Create CommandReproducers on SpanStart an add them to corresponding WhatRanRelevantAction
            if let Some(repro) = CommandReproducer::from_buck_data(&data, &options.options) {
                if let Some(parent_id) = SpanId::from_u64_opt(event.parent_id) {
                    if let Some(entry) = self.known_actions.get_mut(&parent_id) {
                        entry.reproducers.push(repro);
                    }
                }
                return Ok(());
            }
            // Emit WhatRanRelevantAction when we see the corresponding SpanEnd
            if let buck2_data::buck_event::Data::SpanEnd(span) = &data
                && let Some(mut entry) = self
                    .known_actions
                    .shift_remove(&SpanId::from_u64(event.span_id)?)
                && should_emit_finished_action(&span.data, options)
            {
                // Get extra data out of SpanEnd event
                let (execution_kind, std_err, duration, scheduling_mode, remote_command) =
                    match &span.data {
                        Some(buck2_data::span_end_event::Data::ActionExecution(action_exec)) => (
                            Some(action_exec.execution_kind),
                            action_exec.commands.iter().last().and_then(|cmd| {
                                cmd.details.as_ref().map(|d| d.cmd_stderr.as_ref())
                            }),
                            action_exec.wall_time.as_ref().map(
                                |prost_types::Duration { seconds, nanos }| {
                                    std::time::Duration::new(*seconds as u64, *nanos as u32)
                                },
                            ),
                            action_exec
                                .scheduling_mode
                                .as_ref()
                                .and_then(|o| SchedulingMode::try_from(*o).ok()),
                            last_remote_command(action_exec),
                        ),
                        _ => (None, None, None, None, None),
                    };

                if execution_kind == Some(buck2_data::ActionExecutionKind::LocalDepFile as i32)
                    && !options.options.skip_cache_hits
                {
                    entry
                        .reproducers
                        .push(CommandReproducer::LocalDepFileCacheHit);
                }
                if let Some(in_flight) = in_flight_command(&span.data)
                    && !options.options.skip_cache_hits
                {
                    entry
                        .reproducers
                        .push(CommandReproducer::InFlight(in_flight.clone()));
                }
                if let Some(remote) = remote_command {
                    entry.reproducers = entry
                        .reproducers
                        .into_iter()
                        .map(|repro| repro.with_remote_origin(remote))
                        .collect();
                }

                entry.emit_what_ran_entry(output, options, std_err, duration, scheduling_mode)?;
            }
        }

        Ok(())
    }
}

/// The command of an action that took the result of an identical in-flight action.
fn in_flight_command(
    data: &Option<buck2_data::span_end_event::Data>,
) -> Option<&buck2_data::InFlightCommand> {
    let Some(buck2_data::span_end_event::Data::ActionExecution(action_exec)) = data else {
        return None;
    };
    match action_exec
        .commands
        .last()?
        .details
        .as_ref()?
        .command_kind
        .as_ref()?
        .command
        .as_ref()?
    {
        buck2_data::command_execution_kind::Command::InFlightCommand(in_flight) => Some(in_flight),
        _ => None,
    }
}

fn last_remote_command(
    action_exec: &buck2_data::ActionExecutionEnd,
) -> Option<&buck2_data::RemoteCommand> {
    match action_exec
        .commands
        .last()?
        .details
        .as_ref()?
        .command_kind
        .as_ref()?
        .command
        .as_ref()?
    {
        buck2_data::command_execution_kind::Command::RemoteCommand(remote) => Some(remote),
        _ => None,
    }
}

fn should_emit_finished_action(
    data: &Option<buck2_data::span_end_event::Data>,
    options: &WhatRanCommandOptions,
) -> bool {
    if options.incomplete {
        return false;
    }

    match data {
        Some(buck2_data::span_end_event::Data::ActionExecution(action)) => {
            action.failed || !options.failed
        }
        _ => !options.failed, // This is dead code (this span can only be ActionExecution End given
                              // its ID must match an ActionExecution start).
    }
}

fn should_emit_unfinished_action(options: &WhatRanCommandOptions) -> bool {
    !options.failed // We don't know if it failed or not.
}

/// An output that writes to stdout in a tabulated format.
impl WhatRanOutputWriter for OutputFormatWithWriter<'_> {
    fn emit_command(&mut self, command: WhatRanOutputCommand<'_>) -> buck2_error::Result<()> {
        if self.include_std_err && self.omit_empty_std_err && command.std_err == Some("") {
            return Ok(());
        }
        let std_err_formatted = if self.include_std_err {
            Some(command.std_err.map_or_else(
                || "<command did not finish executing>",
                |std_err| {
                    if std_err.is_empty() {
                        "<std_err is empty>"
                    } else {
                        std_err
                    }
                },
            ))
        } else {
            None
        };

        match &mut self.format {
            LogCommandOutputFormatWithWriter::Readable(w)
            | LogCommandOutputFormatWithWriter::Tabulated(w) => {
                w.write_all(format!("{}\n", command.as_tabulated_reproducer()).as_bytes())?;
                if let Some(std_err) = std_err_formatted {
                    write!(
                        w,
                        "{}{}",
                        std_err,
                        if std_err.ends_with('\n') { "" } else { "\n" }
                    )?;
                }
                Ok(())
            }
            LogCommandOutputFormatWithWriter::Json(w) => {
                let reproducer = match &command.repro {
                    CommandReproducer::CacheQuery(cache_hit) => JsonReproducer::CacheQuery {
                        digest: &cache_hit.action_digest,
                    },
                    CommandReproducer::CacheHit(cache_hit) => match cache_hit.cache_type() {
                        buck2_data::CacheType::ActionCache => JsonReproducer::Cache {
                            digest: &cache_hit.action_digest,
                            action_key: cache_hit.action_key.as_deref(),
                        },
                        buck2_data::CacheType::RemoteDepFileCache => {
                            JsonReproducer::ReDepFileCache {
                                digest: &cache_hit.action_digest,
                                action_key: cache_hit.action_key.as_deref(),
                            }
                        }
                    },
                    CommandReproducer::LocalDepFileCacheHit => JsonReproducer::LocalDepFileCache,
                    CommandReproducer::InFlight(in_flight) => JsonReproducer::InFlight {
                        digest: &in_flight.action_digest,
                        leader: in_flight.leader_action_key.as_ref().and_then(|key| {
                            buck2_event_observer::display::display_action_key(
                                key,
                                buck2_event_observer::display::TargetDisplayOptions::for_log(),
                            )
                            .ok()
                        }),
                    },
                    CommandReproducer::ReExecute(re_execute)
                    | CommandReproducer::ReServedFromCache(re_execute)
                    | CommandReproducer::ReDeduplicated(re_execute) => {
                        let not_run = match &command.repro {
                            CommandReproducer::ReServedFromCache(..) => Some("served_from_cache"),
                            CommandReproducer::ReDeduplicated(..) => Some("deduplicated"),
                            _ => None,
                        };
                        if re_execute.persistent_worker {
                            JsonReproducer::ReWorker {
                                digest: &re_execute.action_digest,
                                platform_properties: into_index_map(&re_execute.platform),
                                action_key: re_execute.action_key.as_deref(),
                                not_run,
                            }
                        } else {
                            JsonReproducer::Re {
                                digest: &re_execute.action_digest,
                                platform_properties: into_index_map(&re_execute.platform),
                                action_key: re_execute.action_key.as_deref(),
                                not_run,
                            }
                        }
                    }
                    CommandReproducer::LocalExecute(local_execute) => JsonReproducer::Local {
                        command: local_execute.command.as_ref().map_or_else(
                            || Cow::Owned(Vec::new()),
                            |command| Cow::Borrowed(command.argv.as_ref()),
                        ),
                        env: local_execute
                            .command
                            .as_ref()
                            .into_iter()
                            .flat_map(|command| command.env.iter())
                            .map(|entry| (entry.key.as_ref(), entry.value.as_ref()))
                            .collect(),
                    },
                    CommandReproducer::WorkerExecute(worker_execute) => JsonReproducer::Worker {
                        command: worker_execute.command.as_ref().map_or_else(
                            || Cow::Owned(Vec::new()),
                            |command| Cow::Borrowed(command.argv.as_ref()),
                        ),
                        env: worker_execute
                            .command
                            .as_ref()
                            .into_iter()
                            .flat_map(|command| command.env.iter())
                            .map(|entry| (entry.key.as_ref(), entry.value.as_ref()))
                            .collect(),
                    },

                    // TODO(ctolliday): use the worker_id as the `identity`, and add it to worker execution events.
                    // Currently the identity is the first target that used the worker, which might be misleading.
                    CommandReproducer::WorkerInit(worker_init) => JsonReproducer::WorkerInit {
                        command: worker_init.command.as_ref().map_or_else(
                            || Cow::Owned(Vec::new()),
                            |command| Cow::Borrowed(command.argv.as_ref()),
                        ),
                        env: worker_init
                            .command
                            .as_ref()
                            .into_iter()
                            .flat_map(|command| command.env.iter())
                            .map(|entry| (entry.key.as_ref(), entry.value.as_ref()))
                            .collect(),
                    },
                };
                let std_err = if self.include_std_err {
                    Some(command.std_err.unwrap_or("null"))
                } else {
                    None
                };

                let command = JsonCommand {
                    reason: command.reason,
                    identity: command.identity,
                    reproducer,
                    duration: command.duration.map(fmt_duration::fmt_duration),
                    extra: command.extra.map(Into::into),
                    std_err,
                    scheduling_mode: command.scheduling_mode,
                };
                serde_json::to_writer(w.by_ref(), &command)?;
                w.write_all("\n".as_bytes())?;
                Ok(())
            }
            LogCommandOutputFormatWithWriter::Csv(writer) => {
                #[derive(serde::Serialize)]
                struct Record<'a> {
                    reason: &'a str,
                    identity: &'a str,
                    executor: String,
                    reproducer: String,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    std_err: Option<&'a str>,
                }
                writer
                    .serialize(Record {
                        reason: command.reason,
                        identity: command.identity,
                        executor: command.repro.executor(),
                        reproducer: command.repro.to_string(),
                        std_err: std_err_formatted,
                    })
                    .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::LogCmd))?;
                Ok(())
            }
        }
    }
}

fn into_index_map(platform: &Option<buck2_data::RePlatform>) -> BuckMutInsertionMap<&str, &str> {
    platform
        .as_ref()
        .map_or_else(BuckMutInsertionMap::new, |p| {
            p.properties
                .iter()
                .map(|Property { name, value }| (name.as_ref(), value.as_ref()))
                .collect()
        })
}

#[derive(serde::Serialize)]
struct JsonCommand<'a> {
    reason: &'a str,
    identity: &'a str,
    reproducer: JsonReproducer<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<JsonExtra<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    std_err: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scheduling_mode: Option<SchedulingMode>,
}

mod json_reproducer {
    #![allow(clippy::ref_option_ref)] // within Serialize

    use super::*;

    #[derive(serde::Serialize)]
    #[serde(tag = "executor", content = "details")]
    pub enum JsonReproducer<'a> {
        CacheQuery {
            digest: &'a str,
        },
        Cache {
            digest: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            action_key: Option<&'a str>,
        },
        ReDepFileCache {
            digest: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            action_key: Option<&'a str>,
        },
        LocalDepFileCache,
        InFlight {
            digest: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            leader: Option<String>,
        },
        Re {
            digest: &'a str,
            platform_properties: BuckMutInsertionMap<&'a str, &'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            action_key: Option<&'a str>,
            /// Why the remote execution service did not run the command, if it did not.
            #[serde(skip_serializing_if = "Option::is_none")]
            not_run: Option<&'static str>,
        },
        ReWorker {
            digest: &'a str,
            platform_properties: BuckMutInsertionMap<&'a str, &'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            action_key: Option<&'a str>,
            /// Why the remote execution service did not run the command, if it did not.
            #[serde(skip_serializing_if = "Option::is_none")]
            not_run: Option<&'static str>,
        },
        Local {
            command: Cow<'a, [String]>,
            env: BuckMutInsertionMap<&'a str, &'a str>,
        },
        Worker {
            command: Cow<'a, [String]>,
            env: BuckMutInsertionMap<&'a str, &'a str>,
        },
        WorkerInit {
            command: Cow<'a, [String]>,
            env: BuckMutInsertionMap<&'a str, &'a str>,
        },
    }
}

use json_reproducer::JsonReproducer;

#[derive(serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum JsonExtra<'a> {
    TestCases(&'a [String]),
}

impl<'a> From<WhatRanOutputCommandExtra<'a>> for JsonExtra<'a> {
    fn from(extra: WhatRanOutputCommandExtra<'a>) -> JsonExtra<'a> {
        match extra {
            WhatRanOutputCommandExtra::TestCases(cases) => JsonExtra::TestCases(cases),
        }
    }
}

#[cfg(test)]
mod tests {
    use buck2_event_observer::what_ran::WhatRanOptions;
    use buck2_event_observer::what_ran::WhatRanOutputCommand;
    use buck2_event_observer::what_ran::WhatRanOutputWriter;

    use super::*;
    use crate::what_ran::WhatRanCommandOptions;
    use crate::what_ran::WhatRanCommandState;

    fn make_base_command() -> JsonCommand<'static> {
        let command = Cow::Owned(vec!["some".to_owned(), "command".to_owned()]);
        let mut env = BuckMutInsertionMap::default();
        env.insert("KEY", "val");

        JsonCommand {
            reason: "test.run",
            identity: "some/target",
            reproducer: JsonReproducer::Local { command, env },
            duration: Some("1".to_owned()),
            extra: None,
            std_err: None,
            scheduling_mode: None,
        }
    }

    fn make_base_command_in_re() -> JsonCommand<'static> {
        JsonCommand {
            reason: "test.run",
            identity: "some/target",
            reproducer: JsonReproducer::Re {
                digest: "placeholder",
                platform_properties: buck2_hash::buck_mut_insertion_map! {
                    "platform" => "linux-remote-execution"
                },
                action_key: None,
                not_run: None,
            },
            duration: Some("1".to_owned()),
            extra: None,
            std_err: None,
            scheduling_mode: None,
        }
    }

    #[test]
    fn serialize_what_ran_command_no_extr() -> buck2_error::Result<()> {
        let command = make_base_command();

        let expected = r#"{
  "reason": "test.run",
  "identity": "some/target",
  "reproducer": {
    "executor": "Local",
    "details": {
      "command": [
        "some",
        "command"
      ],
      "env": {
        "KEY": "val"
      }
    }
  },
  "duration": "1"
}"#;
        assert_eq!(expected, serde_json::to_string_pretty(&command)?);
        Ok(())
    }

    #[test]
    fn serialize_what_ran_command_with_extra() -> buck2_error::Result<()> {
        let cases = ["case".to_owned()];
        let mut command = make_base_command();
        command.extra = Some(JsonExtra::TestCases(&cases));

        let expected = r#"{
  "reason": "test.run",
  "identity": "some/target",
  "reproducer": {
    "executor": "Local",
    "details": {
      "command": [
        "some",
        "command"
      ],
      "env": {
        "KEY": "val"
      }
    }
  },
  "duration": "1",
  "extra": {
    "testcases": [
      "case"
    ]
  }
}"#;
        assert_eq!(expected, serde_json::to_string_pretty(&command)?);
        Ok(())
    }

    #[test]
    fn serialize_what_ran_command_in_re() -> buck2_error::Result<()> {
        let command = make_base_command_in_re();

        let expected = r#"{
  "reason": "test.run",
  "identity": "some/target",
  "reproducer": {
    "executor": "Re",
    "details": {
      "digest": "placeholder",
      "platform_properties": {
        "platform": "linux-remote-execution"
      }
    }
  },
  "duration": "1"
}"#;
        assert_eq!(expected, serde_json::to_string_pretty(&command)?);
        Ok(())
    }

    /// Collects `(identity, executor)` of every emitted command.
    #[derive(Default)]
    struct CollectCommands(Vec<(String, String)>);

    impl WhatRanOutputWriter for CollectCommands {
        fn emit_command(&mut self, command: WhatRanOutputCommand<'_>) -> buck2_error::Result<()> {
            self.0
                .push((command.identity.to_owned(), command.repro.executor()));
            Ok(())
        }
    }

    fn span_event(
        span: u64,
        parent: u64,
        data: buck2_data::buck_event::Data,
    ) -> buck2_data::BuckEvent {
        buck2_data::BuckEvent {
            span_id: span,
            parent_id: parent,
            data: Some(data),
            ..Default::default()
        }
    }

    fn action_start_event() -> buck2_data::buck_event::Data {
        buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
            data: Some(buck2_data::span_start_event::Data::ActionExecution(
                buck2_data::ActionExecutionStart {
                    key: Some(buck2_data::ActionKey {
                        owner: Some(buck2_data::action_key::Owner::TargetLabel(
                            buck2_data::ConfiguredTargetLabel {
                                label: Some(buck2_data::TargetLabel {
                                    package: "root//pkg".to_owned(),
                                    name: "t".to_owned(),
                                }),
                                configuration: Some(buck2_data::Configuration {
                                    full_name: "cfg".to_owned(),
                                }),
                                execution_configuration: None,
                            },
                        )),
                        ..Default::default()
                    }),
                    name: Some(buck2_data::ActionName {
                        category: "cat".to_owned(),
                        identifier: "id".to_owned(),
                    }),
                    kind: buck2_data::ActionKind::Run as i32,
                },
            )),
        })
    }

    fn action_end_event(
        execution_kind: buck2_data::ActionExecutionKind,
    ) -> buck2_data::buck_event::Data {
        buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
            data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                buck2_data::ActionExecutionEnd {
                    kind: buck2_data::ActionKind::Run as i32,
                    execution_kind: execution_kind as i32,
                    ..Default::default()
                },
            ))),
            ..Default::default()
        })
    }

    fn test_run_start_event(name: &str) -> buck2_data::buck_event::Data {
        buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
            data: Some(buck2_data::span_start_event::Data::TestRun(
                buck2_data::TestRunStart {
                    suite: Some(buck2_data::TestSuite {
                        suite_name: name.to_owned(),
                        ..Default::default()
                    }),
                },
            )),
        })
    }

    fn local_execute_start_event() -> buck2_data::buck_event::Data {
        buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
            data: Some(buck2_data::span_start_event::Data::ExecutorStage(
                buck2_data::ExecutorStageStart {
                    stage: Some(buck2_data::executor_stage_start::Stage::Local(
                        buck2_data::LocalStage {
                            stage: Some(buck2_data::local_stage::Stage::Execute(
                                buck2_data::LocalExecute::default(),
                            )),
                        },
                    )),
                },
            )),
        })
    }

    fn run_what_ran(
        events: Vec<buck2_data::BuckEvent>,
        options: &WhatRanCommandOptions,
    ) -> buck2_error::Result<Vec<(String, String)>> {
        let mut state = WhatRanCommandState::default();
        let mut out = CollectCommands::default();
        for e in events {
            state.event(Box::new(e), &mut out, options)?;
        }
        state.emit_remaining(&mut out, options)?;
        Ok(out.0)
    }

    /// A local dep-file cache hit is a cache hit, so `--skip-cache-hits` drops it.
    #[test]
    fn test_skip_cache_hits_skips_local_dep_file_hits() -> buck2_error::Result<()> {
        let events = || {
            vec![
                span_event(1, 0, action_start_event()),
                span_event(
                    1,
                    0,
                    action_end_event(buck2_data::ActionExecutionKind::LocalDepFile),
                ),
            ]
        };
        let executors = |options: &WhatRanCommandOptions| -> buck2_error::Result<Vec<String>> {
            Ok(run_what_ran(events(), options)?
                .into_iter()
                .map(|(_, e)| e)
                .collect())
        };

        let options = WhatRanCommandOptions {
            options: WhatRanOptions::default(),
            failed: false,
            incomplete: false,
        };
        assert_eq!(vec!["dep_file".to_owned()], executors(&options)?);

        let options = WhatRanCommandOptions {
            options: WhatRanOptions {
                skip_cache_hits: true,
                ..Default::default()
            },
            failed: false,
            incomplete: false,
        };
        assert_eq!(Vec::<String>::new(), executors(&options)?);
        Ok(())
    }

    /// Commands of spans that never ended are printed in the order the spans started.
    #[test]
    fn test_incomplete_commands_in_start_order() -> buck2_error::Result<()> {
        let options = WhatRanCommandOptions {
            options: WhatRanOptions::default(),
            failed: false,
            incomplete: true,
        };
        let mut events = Vec::new();
        for i in 0..64u64 {
            let span = 2 * i + 1;
            events.push(span_event(span, 0, test_run_start_event(&format!("s{i}"))));
            events.push(span_event(span + 1, span, local_execute_start_event()));
        }
        let identities: Vec<String> = run_what_ran(events, &options)?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let start_order: Vec<String> = (0..64).map(|i| format!("s{i}")).collect();

        assert_eq!(start_order, identities);
        Ok(())
    }
}
