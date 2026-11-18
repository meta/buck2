/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_data::GlobalExternalConfig;
use buck2_error::conversion::from_any_with_tag;
use buck2_event_log::stream_value::StreamValue;
use serde::Serialize;
use tokio_stream::StreamExt;

use crate::LogCommandOutputFormat;
use crate::LogCommandOutputFormatWithWriter;
use crate::transform_format;

const CLI: &str = "cli";
/// Display the values and origins of external configs for a selected command.
///
/// Buckconfigs are computed by joining together values from various inputs (repo, well-known directories, CLI flags). Each of these is
/// logged in the given order, with later components overriding earlier ones. For config files originating from the repo (i.e. project-relative paths), except .buckconfig.local,
/// we log the path, not the actual values.
#[derive(Debug, clap::Parser)]
pub struct ExternalConfigsCommand {
    #[clap(flatten)]
    event_log: EventLogOptions,
    #[clap(flatten)]
    format: LogCommandOutputFormat,
}

impl BuckSubcommand for ExternalConfigsCommand {
    const COMMAND_NAME: &'static str = "log-external-configs";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let Self { event_log, format } = self;

        let log_path = event_log.get(&ctx).await?;

        let (invocation, mut events) = log_path.unpack_stream().await?;
        buck2_client_ctx::eprintln!(
            "Showing external configs from: {}",
            invocation.display_command_line()
        )?;

        while let Some(event) = events.try_next().await? {
            if let StreamValue::Event(event) = event
                && let Some(buck2_data::buck_event::Data::Instant(instant)) = event.data
                && let Some(buck2_data::instant_event::Data::BuckconfigInputValues(configs)) =
                    instant.data
            {
                log_external_configs(&configs.components, format.clone()).await?;
            }
        }

        ExitResult::success()
    }
}

#[derive(Serialize)]
struct ExternalConfigValueEntry<'a> {
    section: &'a str,
    key: &'a str,
    value: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cell: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<&'a str>,
}

#[derive(Serialize)]
struct ExternalConfigFileEntry<'a> {
    path: &'a str,
    origin: &'a str,
}

/// One csv row shape for config files and values. The csv writer takes the header from the
/// first row and rejects rows of another width, so every column is always present.
#[derive(Serialize)]
struct ExternalConfigCsvRow<'a> {
    path: &'a str,
    section: &'a str,
    key: &'a str,
    value: &'a str,
    cell: &'a str,
    origin: &'a str,
}

impl ExternalConfigValueEntry<'_> {
    fn csv_row(&self) -> ExternalConfigCsvRow<'_> {
        ExternalConfigCsvRow {
            path: "",
            section: self.section,
            key: self.key,
            value: self.value,
            cell: self.cell.unwrap_or_default(),
            origin: self.origin.unwrap_or_default(),
        }
    }
}

impl ExternalConfigFileEntry<'_> {
    fn csv_row(&self) -> ExternalConfigCsvRow<'_> {
        ExternalConfigCsvRow {
            path: self.path,
            section: "",
            key: "",
            value: "",
            cell: "",
            origin: self.origin,
        }
    }
}

fn write_config_value<'a>(
    config_value: &'a buck2_data::ConfigValue,
    origin_path: &'a str,
    log_writer: &mut LogCommandOutputFormatWithWriter,
) -> buck2_error::Result<()> {
    let external_config = ExternalConfigValueEntry {
        section: &config_value.section,
        key: &config_value.key,
        value: &config_value.value,
        cell: config_value.cell.as_deref(),
        origin: Some(origin_path),
    };

    match log_writer {
        LogCommandOutputFormatWithWriter::Readable(writer)
        | LogCommandOutputFormatWithWriter::Tabulated(writer) => {
            writeln!(
                writer,
                "{}.{} = {}\t{}\t{}",
                external_config.section,
                external_config.key,
                external_config.value,
                external_config
                    .cell
                    .map_or("".to_owned(), |cell| format!("({cell})")),
                external_config.origin.unwrap_or_default(),
            )?;
        }
        LogCommandOutputFormatWithWriter::Json(writer) => {
            serde_json::to_writer(&mut **writer, &external_config)?;
            writer.write_all("\n".as_bytes())?;
        }
        LogCommandOutputFormatWithWriter::Csv(writer) => {
            writer
                .serialize(external_config.csv_row())
                .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::LogCmd))?;
        }
    }
    Ok(())
}

fn write_config_values(
    global_external_config: &GlobalExternalConfig,
    log_writer: &mut LogCommandOutputFormatWithWriter,
) -> buck2_error::Result<()> {
    global_external_config
        .values
        .iter()
        .try_for_each(|config_value| {
            write_config_value(
                config_value,
                &global_external_config.origin_path,
                log_writer,
            )
        })
}

fn write_config_file(
    path: &str,
    log_writer: &mut LogCommandOutputFormatWithWriter,
) -> buck2_error::Result<()> {
    let origin = "config-file";
    let config_file = ExternalConfigFileEntry { path, origin };
    match log_writer {
        LogCommandOutputFormatWithWriter::Readable(writer)
        | LogCommandOutputFormatWithWriter::Tabulated(writer) => {
            writeln!(writer, "{path}\t\t{origin}")?;
        }
        LogCommandOutputFormatWithWriter::Json(writer) => {
            serde_json::to_writer(&mut **writer, &config_file)?;
            writer.write_all("\n".as_bytes())?;
        }
        LogCommandOutputFormatWithWriter::Csv(writer) => {
            writer
                .serialize(config_file.csv_row())
                .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::LogCmd))?;
        }
    }
    Ok(())
}

async fn log_external_configs(
    components: &[buck2_data::BuckconfigComponent],
    format: LogCommandOutputFormat,
) -> buck2_error::Result<()> {
    buck2_client_ctx::stdio::print_with_writer::<buck2_error::Error, _>(async move |w| {
        let mut log_writer = transform_format(format, w);

        for component in components {
            use buck2_data::buckconfig_component::Data;
            use buck2_data::config_file::Data as CData;
            match &component.data {
                Some(Data::ConfigValue(config_value)) => {
                    assert!(
                        config_value.is_cli,
                        "Only false for configs coming from global external configs which have their origin set below"
                    );
                    write_config_value(config_value, CLI, &mut log_writer)?;
                }
                Some(Data::ConfigFile(config_file)) => config_file
                    .data
                    .as_ref()
                    .into_iter()
                    .try_for_each(|data| match data {
                        CData::ProjectRelativePath(p) => write_config_file(p, &mut log_writer),
                        CData::GlobalExternalConfig(external_config_values) => {
                            write_config_values(external_config_values, &mut log_writer)
                        }
                    })?,

                Some(Data::GlobalExternalConfigFile(external_config_file)) => {
                    write_config_values(external_config_file, &mut log_writer)?
                }
                _ => {}
            }
        }
        Ok(())
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Config files and values share one csv row shape, so any sequence of them writes.
    #[test]
    fn test_csv_rows_share_one_shape() {
        let mut out = Vec::new();
        {
            let mut w = csv::WriterBuilder::new().from_writer(&mut out);
            w.serialize(
                ExternalConfigFileEntry {
                    path: "x.bcfg",
                    origin: "config-file",
                }
                .csv_row(),
            )
            .unwrap();
            w.serialize(
                ExternalConfigValueEntry {
                    section: "a",
                    key: "b",
                    value: "1",
                    cell: None,
                    origin: Some("cli"),
                }
                .csv_row(),
            )
            .unwrap();
            w.serialize(
                ExternalConfigValueEntry {
                    section: "a",
                    key: "c",
                    value: "2",
                    cell: Some("root"),
                    origin: Some("cli"),
                }
                .csv_row(),
            )
            .unwrap();
        }
        let out = String::from_utf8(out).unwrap();
        assert_eq!(
            out,
            "path,section,key,value,cell,origin\nx.bcfg,,,,,config-file\n,a,b,1,,cli\n,a,c,2,root,cli\n"
        );
    }
}
