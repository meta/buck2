/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;
use std::io::Write;

use async_trait::async_trait;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::config::AuditConfigCommand;
use buck2_cmd_audit_client::config::LocationStyle;
use buck2_cmd_audit_client::config::OutputFormat;
use buck2_cmd_audit_client::config::ValueStyle;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::configs::LegacyBuckConfigLocation;
use buck2_common::legacy_configs::configs::LegacyBuckConfigValue;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::name::CellName;
use buck2_hash::BuckMutMap;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::stdout_partial_output::StdoutPartialOutput;
use dupe::Dupe;
use serde_json::json;

use crate::ServerAuditSubcommand;

fn print_location_string(
    writer: &mut impl Write,
    location: &LegacyBuckConfigLocation,
    keyword: &str,
) -> buck2_error::Result<()> {
    writeln!(writer, "  ({keyword} {location})")?;
    Ok(())
}

fn print_location(
    writer: &mut impl Write,
    value: &LegacyBuckConfigValue,
    style: LocationStyle,
) -> buck2_error::Result<()> {
    match style {
        LocationStyle::None => {}
        LocationStyle::Direct => {
            let location = value.location();
            print_location_string(writer, &location, "defined")?;
        }
        LocationStyle::Extended => {
            let stack = value.location_stack();
            let mut iter = stack.iter();
            if let Some(location) = iter.next() {
                // Extra space in the keyword as to align "defined" and "included"
                print_location_string(writer, location, "defined ")?;
            }
            for location in iter {
                print_location_string(writer, location, "included")?;
            }
        }
    }

    Ok(())
}

fn print_value(
    writer: &mut impl Write,
    inline_section: Option<&str>,
    key: &str,
    value: &LegacyBuckConfigValue,
    style: ValueStyle,
) -> buck2_error::Result<()> {
    let (prefix, sep) = match inline_section {
        Some(section) => (section, "."),
        None => ("    ", ""),
    };
    match style {
        ValueStyle::Resolved => {
            writeln!(writer, "{}{}{} = {}", prefix, sep, key, value.as_str())?;
        }
        ValueStyle::Raw => {
            writeln!(writer, "{}{}{} = {}", prefix, sep, key, value.raw_value())?;
        }
        ValueStyle::Both => {
            writeln!(
                writer,
                "{}{}{} = {}\n        (raw {})",
                prefix,
                sep,
                key,
                value.as_str(),
                value.raw_value()
            )?;
        }
    }

    Ok(())
}

struct Match<'a> {
    /// The original specification - important if we want to produce JSON keys.
    /// Not present if it wasn't a key match, as then we can't express it.
    spec: Option<&'a str>,
    /// The cell, or otherwise require the cell passed in by the filterer.
    cell: Option<CellName>,
    /// The section.
    section: &'a str,
    /// The key. Might be optional if the user did `foo` as their match.
    key: Option<&'a str>,
}

impl<'a> Match<'a> {
    fn parse(resolver: &CellAliasResolver, spec: &'a str) -> buck2_error::Result<Self> {
        let (cell, config) = match spec.split_once("//") {
            Some((cell, config)) => (Some(resolver.resolve(cell)?), config),
            None => (None, spec),
        };
        let (section, key) = config.split_once(".").unwrap_or((config, ""));
        Ok(Self {
            spec: if key == "" { None } else { Some(spec) },
            cell,
            section,
            key: if key == "" { None } else { Some(key) },
        })
    }

    fn filter(
        &self,
        default_cell: CellName,
        cell: CellName,
        section: &str,
        key: &str,
    ) -> Option<String> {
        if cell == self.cell.unwrap_or(default_cell)
            && section == self.section
            && self.key.is_none_or(|k| k == key)
        {
            Some(
                self.spec
                    .map_or_else(|| format!("{section}.{key}"), str::to_owned),
            )
        } else {
            None
        }
    }
}

struct Matches<'a> {
    matches: Vec<Match<'a>>,
}

impl<'a> Matches<'a> {
    fn parse(resolver: &CellAliasResolver, specs: &'a [String]) -> buck2_error::Result<Self> {
        Ok(Self {
            matches: specs
                .iter()
                .map(|x| Match::parse(resolver, x))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    fn filter(
        &self,
        default_cell: CellName,
        cell: CellName,
        section: &str,
        key: &str,
    ) -> Option<String> {
        if self.matches.is_empty() {
            if cell == default_cell {
                Some(format!("{section}.{key}"))
            } else {
                None
            }
        } else {
            self.matches
                .iter()
                .find_map(|x| x.filter(default_cell, cell, section, key))
        }
    }

    fn cells(&self) -> BTreeSet<CellName> {
        self.matches.iter().filter_map(|x| x.cell).collect()
    }
}

trait CellConfigRenderer {
    fn render_cell_header(&mut self, cell: CellName) -> buck2_error::Result<()>;
    fn render_section_header(&mut self, section: &str) -> buck2_error::Result<()>;
    fn render_config_key(
        &mut self,
        spec: &str,
        cell: CellName,
        section: &str,
        key: &str,
        value: LegacyBuckConfigValue<'_>,
    ) -> buck2_error::Result<()>;
    fn flush(&mut self) -> buck2_error::Result<()>;
}

struct SimpleCellConfigRenderer<'a> {
    stdout: StdoutPartialOutput<'a>,
    render_cell_headers: bool,
    value_style: ValueStyle,
    location_style: LocationStyle,
    inline_section: bool,
}

impl CellConfigRenderer for SimpleCellConfigRenderer<'_> {
    fn render_cell_header(&mut self, cell: CellName) -> buck2_error::Result<()> {
        if self.render_cell_headers {
            writeln!(&mut self.stdout, "# Cell: {cell}")?;
        }

        Ok(())
    }

    fn render_section_header(&mut self, section: &str) -> buck2_error::Result<()> {
        if !self.inline_section {
            writeln!(&mut self.stdout, "[{section}]")?;
        }

        Ok(())
    }

    fn render_config_key(
        &mut self,
        _spec: &str,
        _cell: CellName,
        section: &str,
        key: &str,
        value: LegacyBuckConfigValue<'_>,
    ) -> buck2_error::Result<()> {
        let inline_section = if self.inline_section {
            Some(section)
        } else {
            None
        };
        print_value(
            &mut self.stdout,
            inline_section,
            key,
            &value,
            self.value_style,
        )?;
        print_location(&mut self.stdout, &value, self.location_style)?;

        Ok(())
    }

    fn flush(&mut self) -> buck2_error::Result<()> {
        Ok(())
    }
}

struct JsonCellConfigRenderer<'a> {
    stdout: StdoutPartialOutput<'a>,
    scope_keys_to_cell: bool,
    json_output: BuckMutMap<String, String>,
}

impl CellConfigRenderer for JsonCellConfigRenderer<'_> {
    fn render_cell_header(&mut self, _cell: CellName) -> buck2_error::Result<()> {
        Ok(())
    }

    fn render_section_header(&mut self, _section: &str) -> buck2_error::Result<()> {
        Ok(())
    }

    fn render_config_key(
        &mut self,
        spec: &str,
        cell: CellName,
        _section: &str,
        _key: &str,
        value: LegacyBuckConfigValue<'_>,
    ) -> buck2_error::Result<()> {
        let key = if self.scope_keys_to_cell && !spec.contains("//") {
            format!("{cell}//{spec}")
        } else {
            spec.to_owned()
        };

        self.json_output.insert(key, value.as_str().to_owned());

        Ok(())
    }

    fn flush(&mut self) -> buck2_error::Result<()> {
        writeln!(&mut self.stdout, "{}", json!(self.json_output))?;

        Ok(())
    }
}

fn render_cell_config(
    renderer: &mut dyn CellConfigRenderer,
    relevant_cell: Option<CellName>,
    cell: CellName,
    cell_config: LegacyBuckConfig,
    specs: &Matches<'_>,
) -> buck2_error::Result<()> {
    let mut rendered_cell_header = false;
    for (section, values) in cell_config.all_sections() {
        let mut rendered_section_header = false;
        for (key, value) in values.iter() {
            if let Some(spec) = specs.filter(relevant_cell.unwrap_or(cell), cell, section, key) {
                if !rendered_cell_header {
                    renderer.render_cell_header(cell)?;
                    rendered_cell_header = true;
                }

                if !rendered_section_header {
                    renderer.render_section_header(section.as_str())?;
                    rendered_section_header = true;
                }

                renderer.render_config_key(&spec, cell, section, key, value)?;
            }
        }
    }

    Ok(())
}

#[async_trait]
impl ServerAuditSubcommand for AuditConfigCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        Ok(server_ctx
            .with_dice_ctx(|server_ctx, ctx| async move {
                let cwd = server_ctx.working_dir();
                let cell_resolver = ctx.ctx().get_cell_resolver().await?;
                let cell_alias_resolver = ctx.ctx().get_cell_alias_resolver_for_dir(cwd).await?;

                let stdout = stdout.as_writer();
                let mut renderer: Box<dyn CellConfigRenderer + Send> = match self.output_format() {
                    OutputFormat::Simple => Box::new(SimpleCellConfigRenderer {
                        stdout,
                        render_cell_headers: self.all_cells,
                        value_style: self.value_style,
                        location_style: self.location_style,
                        inline_section: false,
                    }),
                    OutputFormat::InlineSection => Box::new(SimpleCellConfigRenderer {
                        stdout,
                        render_cell_headers: self.all_cells,
                        value_style: self.value_style,
                        location_style: self.location_style,
                        inline_section: true,
                    }),
                    OutputFormat::Json => Box::new(JsonCellConfigRenderer {
                        stdout,
                        scope_keys_to_cell: self.all_cells,
                        json_output: BuckMutMap::default(),
                    }),
                };

                let specs = Matches::parse(&cell_alias_resolver, &self.specs)?;

                if self.all_cells {
                    for (cell, _) in cell_resolver.cells() {
                        let cell_config = ctx.ctx().get_legacy_config_for_cell(cell).await?.dupe();
                        render_cell_config(renderer.as_mut(), None, cell, cell_config, &specs)?;
                    }
                } else {
                    let cell =
                        cell_alias_resolver.resolve(self.cell.as_deref().unwrap_or_default())?;

                    {
                        // Render the target cell first
                        let cell_config = ctx.ctx().get_legacy_config_for_cell(cell).await?.dupe();
                        render_cell_config(
                            renderer.as_mut(),
                            Some(cell),
                            cell,
                            cell_config,
                            &specs,
                        )?;
                    }

                    // Allow callers to specify a "cell//<foo>" spec without --all-cells
                    let mut cells_to_render = specs.cells();
                    cells_to_render.remove(&cell);
                    let relevant_cell = Some(cell);

                    for cell in cells_to_render {
                        let cell_config = ctx.ctx().get_legacy_config_for_cell(cell).await?.dupe();
                        render_cell_config(
                            renderer.as_mut(),
                            relevant_cell,
                            cell,
                            cell_config,
                            &specs,
                        )?;
                    }
                }

                renderer.flush()
            })
            .await?)
    }
}
