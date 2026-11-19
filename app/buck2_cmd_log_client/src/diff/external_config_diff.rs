/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::fmt::Display;

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_event_log::stream_value::StreamValue;
use futures::Stream;
use futures::TryStreamExt;
use serde::Serialize;
use similar::ChangeTag;
use similar::TextDiff;

use crate::diff::diff_options::DiffEventLogOptions;

const PROJECT_ROOT: &str = "";

/// Output format options for log diff external-config.
///
/// Determines how the command output is formatted and displayed.
#[derive(Debug, Clone, clap::ValueEnum)]
#[clap(rename_all = "snake_case")]
enum ExternalConfigDiffFormat {
    /// Human-readable output (default).
    Readable,
    /// JSON format, one object per line.
    Json,
}

/// Identifies the diff between external buckconfigs between two commands.
#[derive(Debug, clap::Parser)]
pub struct ExternalConfigDiffCommand {
    #[clap(flatten)]
    diff_event_log: DiffEventLogOptions,
    #[clap(
        long,
        help = "Which output format to use for this command",
        default_value = "readable",
        ignore_case = true,
        value_enum
    )]
    format: ExternalConfigDiffFormat,
}

fn insert_config_value(
    dict: &mut BTreeMap<String, String>,
    order: &mut Vec<String>,
    config: &buck2_data::ConfigValue,
) {
    let config_cell = config
        .cell
        .clone()
        .map_or(PROJECT_ROOT.to_owned(), |cell| format!("({cell})"));
    let key = format!(
        "{}{}.{}",
        config_cell,
        config.section.clone(),
        config.key.clone()
    );
    order.push(format!("{key}={}", config.value));
    dict.insert(key, config.value.clone());
}

fn insert_config_values(
    dict: &mut BTreeMap<String, String>,
    order: &mut Vec<String>,
    configs: &[buck2_data::ConfigValue],
) {
    configs
        .iter()
        .for_each(|config_value| insert_config_value(dict, order, config_value))
}

fn process_buckconfig_data(
    dict: &mut BTreeMap<String, String>,
    order: &mut Vec<String>,
    event: &buck2_data::BuckEvent,
) {
    use buck2_data::buckconfig_component::Data::ConfigFile;
    use buck2_data::buckconfig_component::Data::ConfigValue;
    use buck2_data::buckconfig_component::Data::GlobalExternalConfigFile;
    use buck2_data::config_file::Data::GlobalExternalConfig;
    use buck2_data::config_file::Data::ProjectRelativePath;

    if let Some(buck2_data::buck_event::Data::Instant(end)) = event.data.as_ref() {
        if let Some(buck2_data::instant_event::Data::BuckconfigInputValues(input)) =
            end.data.as_ref()
        {
            input
                .components
                .iter()
                .for_each(|component| match component.data.as_ref() {
                    Some(ConfigValue(config_value)) => {
                        insert_config_value(dict, order, config_value)
                    }
                    Some(ConfigFile(config_file)) => config_file
                        .data
                        .as_ref()
                        .into_iter()
                        .for_each(|data| match data {
                            ProjectRelativePath(p) => {
                                order.push(p.clone());
                                dict.insert(p.clone(), "".to_owned());
                            }
                            GlobalExternalConfig(external_config_values) => {
                                insert_config_values(dict, order, &external_config_values.values)
                            }
                        }),
                    Some(GlobalExternalConfigFile(external_config_file)) => {
                        insert_config_values(dict, order, &external_config_file.values)
                    }
                    _ => {}
                });
        }
    }
}

async fn get_external_buckconfig_dict(
    mut events: impl Stream<Item = buck2_error::Result<StreamValue>> + Unpin + Send,
) -> buck2_error::Result<(BTreeMap<String, String>, Vec<String>)> {
    let mut dict: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    while let Some(event) = events.try_next().await? {
        if let StreamValue::Event(event) = event {
            process_buckconfig_data(&mut dict, &mut order, &event);
        }
    }
    Ok((dict, order))
}

#[derive(Debug, Hash, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum DiffTag {
    Equal,
    Delete,
    Insert,
}

#[derive(Debug, Hash, PartialEq, Eq, Serialize)]
struct DiffChange {
    tag: DiffTag,
    value: String,
}

#[derive(Debug, Hash, PartialEq, Eq, Serialize)]
enum DiffType<'a> {
    Changed {
        key: &'a str,
        old_value: &'a str,
        new_value: &'a str,
    },
    FirstOnly {
        key: &'a str,
        value: &'a str,
    },
    SecondOnly {
        key: &'a str,
        value: &'a str,
    },
    FullDiff {
        changes: Vec<DiffChange>,
    },
}

impl Display for DiffType<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffType::Changed {
                key,
                old_value,
                new_value,
            } => write!(f, "{key}: {old_value} | {new_value}"),
            DiffType::FirstOnly { key, value } => write!(f, "{key}: {value} | _"),
            DiffType::SecondOnly { key, value } => write!(f, "{key}: _ | {value}"),
            DiffType::FullDiff { changes } => {
                writeln!(f, "\n=== Full Diff ===")?;
                for change in changes {
                    let sign = match change.tag {
                        DiffTag::Delete => "-",
                        DiffTag::Insert => "+",
                        DiffTag::Equal => " ",
                    };
                    writeln!(f, "{sign} {}", change.value)?;
                }
                Ok(())
            }
        }
    }
}

impl BuckSubcommand for ExternalConfigDiffCommand {
    const COMMAND_NAME: &'static str = "log-diff-buckconfig";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let Self {
            diff_event_log,
            format,
        } = self;

        let (log_path1, log_path2) = diff_event_log.get(&ctx).await?;

        let (invocation1, events1) = log_path1.unpack_stream().await?;
        let (invocation2, events2) = log_path2.unpack_stream().await?;

        buck2_client_ctx::println!(
            "Identifying the diff of external buckconfigs between: \n{} and \n{}",
            invocation1.display_command_line(),
            invocation2.display_command_line()
        )?;

        // External buckconfigs are stored in the event log in order and can have overrides
        // We first resolve them into a single dict
        let (dict1, order1) = get_external_buckconfig_dict(events1).await?;
        let (dict2, order2) = get_external_buckconfig_dict(events2).await?;
        let mut diffs = Vec::new();
        for (key, value) in dict1.iter() {
            if let Some(new_value) = dict2.get(key) {
                if new_value != value {
                    diffs.push(DiffType::Changed {
                        key,
                        old_value: value,
                        new_value,
                    });
                }
            } else {
                diffs.push(DiffType::FirstOnly { key, value });
            }
        }

        for (key, value) in dict2.iter() {
            if !dict1.contains_key(key) {
                diffs.push(DiffType::SecondOnly { key, value });
            }
        }

        if order1 != order2 {
            let first: Vec<&str> = order1.iter().map(String::as_str).collect();
            let second: Vec<&str> = order2.iter().map(String::as_str).collect();
            let changes = TextDiff::from_slices(&first, &second)
                .iter_all_changes()
                .map(|change| DiffChange {
                    tag: match change.tag() {
                        ChangeTag::Equal => DiffTag::Equal,
                        ChangeTag::Delete => DiffTag::Delete,
                        ChangeTag::Insert => DiffTag::Insert,
                    },
                    value: change.value().to_owned(),
                })
                .collect();
            diffs.push(DiffType::FullDiff { changes });
        }

        buck2_client_ctx::stdio::print_with_writer::<buck2_error::Error, _>(async move |w| {
            match format {
                ExternalConfigDiffFormat::Readable => {
                    writeln!(w, "=== Summary Diff ===")?;
                    for diff in &diffs {
                        writeln!(w, "{diff}")?;
                    }
                }
                ExternalConfigDiffFormat::Json => {
                    for diff in &diffs {
                        serde_json::to_writer(&mut *w, diff)?;
                        writeln!(w)?;
                    }
                }
            }
            Ok(())
        })
        .await?;

        ExitResult::success()
    }
}
