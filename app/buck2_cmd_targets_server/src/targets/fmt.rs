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
use std::fmt::Write;
use std::sync::Arc;

use buck2_build_api::configure_targets::ConfiguredTargetsWithErrors;
use buck2_cli_proto::ConfiguredTargetsRequest;
use buck2_cli_proto::HasClientContext;
use buck2_cli_proto::TargetsRequest;
use buck2_cli_proto::configured_targets_request::OutputFormat as ConfiguredOutputFormat;
use buck2_cli_proto::targets_request;
use buck2_cli_proto::targets_request::OutputFormat;
use buck2_cli_proto::targets_request::TargetHashGraphType;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::configuration::compatibility::IncompatiblePlatformReason;
use buck2_core::configuration::compatibility::IncompatiblePlatformReasonCause;
use buck2_core::package::PackageLabel;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_node::attrs::hacks::configured_value_to_json;
use buck2_node::attrs::hacks::value_to_json;
use buck2_node::attrs::inspect_options::AttrInspectOptions;
use buck2_node::nodes::attributes::DEPS;
use buck2_node::nodes::attributes::INPUTS;
use buck2_node::nodes::attributes::ONCALL;
use buck2_node::nodes::attributes::PACKAGE;
use buck2_node::nodes::attributes::PACKAGE_VALUES;
use buck2_node::nodes::attributes::TARGET_CALL_STACK;
use buck2_node::nodes::attributes::TARGET_HASH;
use buck2_node::nodes::attributes::TYPE;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::unconfigured::TargetNodeRef;
use buck2_node::super_package::SuperPackage;
use buck2_util::indent::indent;
use regex::RegexSet;

use crate::configured_target_hash::ConfiguredTargetHash;
use crate::json::QuotedJson;
use crate::target_hash::BuckTargetHash;

pub(crate) struct TargetInfo<'a> {
    pub(crate) node: TargetNodeRef<'a>,
    pub(crate) target_hash: Option<BuckTargetHash>,
    pub(crate) super_package: &'a SuperPackage,
}

fn package_error_to_stderr(package: PackageLabel, error: &buck2_error::Error, stderr: &mut String) {
    writeln!(stderr, "Error parsing {package}\n{error:?}").unwrap();
}

fn target_error_to_stderr(error: &buck2_error::Error, stderr: &mut String) {
    writeln!(stderr, "ERROR:{:#}\n", error).unwrap();
}

#[allow(unused_variables)]
pub(crate) trait TargetFormatter: Send + Sync {
    fn begin(&self, buffer: &mut String) {}
    fn end(&self, stats: &Stats, buffer: &mut String) {}
    /// Called between each target/imports/package_error
    fn separator(&self, buffer: &mut String) {}
    fn target(&self, target_info: TargetInfo<'_>, buffer: &mut String) {}
    fn imports(
        &self,
        source: &CellPath,
        imports: &[ImportPath],
        package: Option<PackageLabel>,
        buffer: &mut String,
    ) {
    }
    fn package_error(
        &self,
        package: PackageLabel,
        error: &buck2_error::Error,
        stdout: &mut String,
        stderr: &mut String,
    ) {
        package_error_to_stderr(package, error, stderr);
    }
}

#[allow(unused_variables)]
pub trait ConfiguredTargetFormatter: Send + Sync {
    fn begin(&self, buffer: &mut String) {}
    fn end(&self, buffer: &mut String) {}
    fn separator(&self, buffer: &mut String) {}
    fn target(
        &self,
        target_node: &ConfiguredTargetNode,
        target_hash: Option<ConfiguredTargetHash>,
        buffer: &mut String,
    ) -> buck2_error::Result<()> {
        Ok(())
    }
    fn package_error(
        &self,
        package: PackageLabel,
        error: &buck2_error::Error,
        stdout: &mut String,
        stderr: &mut String,
    ) {
        package_error_to_stderr(package, error, stderr);
    }
    fn target_error(&self, error: &buck2_error::Error, stdout: &mut String, stderr: &mut String) {
        target_error_to_stderr(error, stderr);
    }
}

pub(crate) struct JsonWriter {
    pub(crate) json_lines: bool,
}

impl JsonWriter {
    pub(crate) fn begin(&self, buffer: &mut String) {
        if !self.json_lines {
            buffer.push_str("[\n");
        }
    }

    pub(crate) fn end(&self, buffer: &mut String) {
        if !self.json_lines {
            buffer.push_str("\n]\n");
        }
    }

    pub(crate) fn separator(&self, buffer: &mut String) {
        if !self.json_lines {
            buffer.push_str(",\n");
        }
    }

    pub(crate) fn entry_start(&self, buffer: &mut String) {
        if self.json_lines {
            buffer.push('{');
        } else {
            buffer.push_str("  {\n");
        }
    }

    pub(crate) fn entry_end(&self, buffer: &mut String, is_first_entry: bool) {
        if self.json_lines {
            buffer.push_str("}\n");
        } else {
            if !is_first_entry {
                buffer.push('\n');
            }
            buffer.push_str("  }");
        }
    }

    pub(crate) fn entry_item(
        &self,
        buffer: &mut String,
        is_first_entry: &mut bool,
        key: &str,
        value: QuotedJson,
    ) {
        if *is_first_entry {
            *is_first_entry = false;
        } else if self.json_lines {
            buffer.push(',');
        } else {
            buffer.push_str(",\n");
        }
        if !self.json_lines {
            buffer.push_str("    ");
        }
        write!(
            buffer,
            "{}:{}",
            serde_json::to_string(key).unwrap(),
            value.as_str()
        )
        .unwrap();
    }
}

struct JsonFormat {
    attributes: Option<RegexSet>,
    attr_inspect_opts: AttrInspectOptions,
    target_call_stacks: bool,
    package_values: Option<RegexSet>,
    writer: JsonWriter,
}

impl JsonFormat {
    fn print_attr(
        &self,
        buffer: &mut String,
        is_first_entry: &mut bool,
        k: &str,
        v: impl FnOnce() -> QuotedJson,
    ) {
        if let Some(filter) = &self.attributes {
            if !filter.is_match(k) {
                return;
            }
        }
        self.writer.entry_item(buffer, is_first_entry, k, v());
    }
}

impl TargetFormatter for JsonFormat {
    fn begin(&self, buffer: &mut String) {
        self.writer.begin(buffer)
    }

    fn end(&self, _stats: &Stats, buffer: &mut String) {
        self.writer.end(buffer)
    }

    fn separator(&self, buffer: &mut String) {
        self.writer.separator(buffer)
    }

    fn target(&self, target_info: TargetInfo<'_>, buffer: &mut String) {
        self.writer.entry_start(buffer);
        let mut first = true;

        self.print_attr(buffer, &mut first, TYPE, || {
            QuotedJson::quote_str(&target_info.node.rule_type().to_string())
        });
        self.print_attr(buffer, &mut first, DEPS, || {
            QuotedJson::list(target_info.node.deps().map(QuotedJson::quote_display))
        });

        self.print_attr(buffer, &mut first, INPUTS, || {
            QuotedJson::list(target_info.node.inputs().map(QuotedJson::quote_display))
        });

        if let Some(hash) = target_info.target_hash {
            self.print_attr(buffer, &mut first, TARGET_HASH, || {
                QuotedJson::quote_display(hash)
            });
        }
        self.print_attr(buffer, &mut first, PACKAGE, || {
            QuotedJson::quote_display(target_info.node.label().pkg())
        });

        if let Some(filter) = &self.package_values {
            self.print_attr(buffer, &mut first, PACKAGE_VALUES, || {
                let package_values = serde_json::Value::Object(
                    target_info
                        .super_package
                        .package_values()
                        .package_values_json()
                        // TODO(nga): return error.
                        .unwrap()
                        .iter()
                        .filter(|(k, _)| filter.is_match(k.as_str()))
                        .map(|(k, v)| (k.as_str().to_owned(), v.clone()))
                        .collect(),
                );
                QuotedJson::from_serde_json_value(package_values)
            });
        }

        if let Some(oncall) = target_info.node.oncall() {
            self.print_attr(buffer, &mut first, ONCALL, || {
                QuotedJson::quote_display(oncall)
            });
        }

        for a in target_info.node.attrs(self.attr_inspect_opts) {
            self.print_attr(buffer, &mut first, a.name, || {
                QuotedJson::from_serde_json_value(
                    value_to_json(a.value, target_info.node.label().pkg()).unwrap(),
                )
            });
        }

        if self.target_call_stacks {
            match target_info.node.call_stack() {
                Some(call_stack) => {
                    self.print_attr(buffer, &mut first, TARGET_CALL_STACK, || {
                        QuotedJson::quote_str(&call_stack)
                    });
                }
                None => {
                    // Should not happen.
                }
            }
        }

        self.writer.entry_end(buffer, first);
    }

    fn imports(
        &self,
        source: &CellPath,
        imports: &[ImportPath],
        package: Option<PackageLabel>,
        buffer: &mut String,
    ) {
        self.writer.entry_start(buffer);
        let mut first = true;
        if let Some(package) = package {
            self.writer.entry_item(
                buffer,
                &mut first,
                PACKAGE,
                QuotedJson::quote_str(&package.to_string()),
            );
        }
        self.writer.entry_item(
            buffer,
            &mut first,
            "buck.file",
            QuotedJson::quote_str(&source.to_string()),
        );
        self.writer.entry_item(
            buffer,
            &mut first,
            "buck.imports",
            QuotedJson::list(
                imports
                    .iter()
                    .map(|d| QuotedJson::quote_display(d.path()))
                    .collect::<Vec<_>>(),
            ),
        );
        self.writer.entry_end(buffer, first);
    }

    fn package_error(
        &self,
        package: PackageLabel,
        error: &buck2_error::Error,
        stdout: &mut String,
        stderr: &mut String,
    ) {
        // When an error happens we print it to stdout (as a JSON entry) and to stderr (as a human message).
        // If the user has keep-going turned on, they'll get the JSON on stdout, but also have the error message appear on stderr.
        // If the user has keep-going turned off, they'll only see one error message and then abort.
        package_error_to_stderr(package, error, stderr);
        self.writer.entry_start(stdout);
        let mut first = true;
        self.writer.entry_item(
            stdout,
            &mut first,
            PACKAGE,
            QuotedJson::quote_display(package),
        );
        self.writer.entry_item(
            stdout,
            &mut first,
            "buck.error",
            QuotedJson::quote_str(&format!("{error:?}")),
        );
        self.writer.entry_end(stdout, first);
    }
}

impl ConfiguredTargetFormatter for JsonFormat {
    fn begin(&self, buffer: &mut String) {
        self.writer.begin(buffer)
    }

    fn end(&self, buffer: &mut String) {
        self.writer.end(buffer)
    }

    fn separator(&self, buffer: &mut String) {
        self.writer.separator(buffer)
    }

    fn target(
        &self,
        target_node: &ConfiguredTargetNode,
        target_hash: Option<ConfiguredTargetHash>,
        buffer: &mut String,
    ) -> buck2_error::Result<()> {
        self.writer.entry_start(buffer);
        let mut is_first_entry = true;

        self.print_attr(buffer, &mut is_first_entry, "buck.target", || {
            QuotedJson::quote_display(target_node.label().unconfigured())
        });

        self.print_attr(buffer, &mut is_first_entry, TYPE, || {
            QuotedJson::quote_str(&target_node.rule_type().to_string())
        });

        self.print_attr(buffer, &mut is_first_entry, DEPS, || {
            QuotedJson::list(
                target_node
                    .deps()
                    .iter()
                    .map(|n| QuotedJson::quote_display(n.label()))
                    .collect::<Vec<_>>(),
            )
        });

        self.print_attr(buffer, &mut is_first_entry, INPUTS, || {
            QuotedJson::list(target_node.inputs().map(QuotedJson::quote_display))
        });

        if let Some(hash) = target_hash {
            self.print_attr(buffer, &mut is_first_entry, TARGET_HASH, || {
                QuotedJson::quote_display(hash)
            });
        }

        self.print_attr(buffer, &mut is_first_entry, PACKAGE, || {
            QuotedJson::quote_display(target_node.label().pkg())
        });

        if let Some(oncall) = target_node.oncall() {
            self.print_attr(buffer, &mut is_first_entry, ONCALL, || {
                QuotedJson::quote_display(oncall)
            });
        }

        for a in target_node.attrs(self.attr_inspect_opts) {
            let json_value = configured_value_to_json(&a.value, target_node.label().pkg())?;
            self.print_attr(buffer, &mut is_first_entry, a.name, || {
                QuotedJson::from_serde_json_value(json_value)
            });
        }

        if self.target_call_stacks {
            if let Some(call_stack) = target_node.call_stack() {
                self.print_attr(buffer, &mut is_first_entry, TARGET_CALL_STACK, || {
                    QuotedJson::quote_str(&call_stack)
                });
            }
        }

        self.writer.entry_end(buffer, is_first_entry);

        Ok(())
    }

    fn package_error(
        &self,
        package: PackageLabel,
        error: &buck2_error::Error,
        stdout: &mut String,
        stderr: &mut String,
    ) {
        // When an error happens we print it to stdout (as a JSON entry) and to stderr (as a human message).
        // If the user has keep-going turned on, they'll get the JSON on stdout, but also have the error message appear on stderr.
        // If the user has keep-going turned off, they'll only see one error message and then abort.
        package_error_to_stderr(package, error, stderr);
        self.writer.entry_start(stdout);
        let mut first = true;
        self.writer.entry_item(
            stdout,
            &mut first,
            PACKAGE,
            QuotedJson::quote_display(package),
        );
        self.writer.entry_item(
            stdout,
            &mut first,
            "buck.error",
            QuotedJson::quote_str(&format!("{error:?}")),
        );
        self.writer.entry_end(stdout, first);
    }

    fn target_error(&self, error: &buck2_error::Error, stdout: &mut String, stderr: &mut String) {
        // Similar to package_error, output to both stdout (JSON) and stderr (human readable).
        // Target-level errors don't have package context, so we only include the error field.
        target_error_to_stderr(error, stderr);

        self.writer.entry_start(stdout);
        let mut first = true;
        self.writer.entry_item(
            stdout,
            &mut first,
            "buck.error",
            QuotedJson::quote_str(&format!("{error:?}")),
        );
        self.writer.entry_end(stdout, first);
    }
}

#[derive(Debug, Default)]
pub(crate) struct Stats {
    pub(crate) errors: u64,
    error_tags: BTreeSet<buck2_error::ErrorTag>,
    pub(crate) success: u64,
    pub(crate) targets: u64,
}

impl Stats {
    pub(crate) fn merge(&mut self, stats: &Stats) {
        self.errors += stats.errors;
        self.success += stats.success;
        self.targets += stats.targets;
        self.error_tags.extend(stats.error_tags.iter().copied());
    }

    pub(crate) fn add_error(&mut self, e: &buck2_error::Error) {
        self.error_tags.extend(e.tags());
        self.errors += 1;
    }

    pub(crate) fn to_error(&self) -> Option<buck2_error::Error> {
        if self.errors == 0 {
            return None;
        }
        // Simpler error so that we don't print long errors twice (when exiting buck2)
        let package_str = if self.errors == 1 {
            "package"
        } else {
            "packages"
        };

        #[derive(buck2_error::Error, Debug)]
        #[buck2(tag = Input)]
        enum TargetsError {
            #[error("Failed to parse {0} {1}")]
            FailedToParse(u64, &'static str),
        }

        let mut e = buck2_error::Error::from(TargetsError::FailedToParse(self.errors, package_str));
        e = e.tag(self.error_tags.iter().copied());
        Some(e)
    }
}

struct StatsFormat;

impl TargetFormatter for StatsFormat {
    fn end(&self, stats: &Stats, buffer: &mut String) {
        writeln!(buffer, "{stats:?}").unwrap()
    }
}

struct TargetNameFormat {
    target_call_stacks: bool,
    target_hash_graph_type: TargetHashGraphType,
}
impl TargetFormatter for TargetNameFormat {
    fn target(&self, target_info: TargetInfo<'_>, buffer: &mut String) {
        if self.target_hash_graph_type != TargetHashGraphType::None {
            if let Some(hash) = target_info.target_hash {
                writeln!(buffer, "{label} {hash}", label = target_info.node.label()).unwrap()
            }
            // print nothing if there is no hash and show_target_hash is specified.
        } else {
            writeln!(buffer, "{}", target_info.node.label()).unwrap();
        }
        if self.target_call_stacks {
            print_target_call_stack_after_target(buffer, target_info.node.call_stack().as_deref());
        }
    }
}

impl ConfiguredTargetFormatter for TargetNameFormat {
    fn target(
        &self,
        target_node: &ConfiguredTargetNode,
        target_hash: Option<ConfiguredTargetHash>,
        buffer: &mut String,
    ) -> buck2_error::Result<()> {
        match target_hash {
            Some(hash) => writeln!(buffer, "{} {}", target_node.label(), hash),
            None => writeln!(buffer, "{}", target_node.label()),
        }
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?;
        if self.target_call_stacks {
            print_target_call_stack_after_target(buffer, target_node.call_stack().as_deref());
        };

        Ok(())
    }
}

pub(crate) fn print_target_call_stack_after_target(out: &mut String, call_stack: Option<&str>) {
    if let Some(call_stack) = call_stack {
        write!(out, "{}", indent("  ", call_stack)).unwrap();
    }
}

pub struct JsonReportFormat {
    json_format: JsonFormat,
}

impl JsonReportFormat {
    pub fn new(
        attributes: Option<RegexSet>,
        attr_inspect_opts: AttrInspectOptions,
        target_call_stacks: bool,
    ) -> Self {
        Self {
            json_format: JsonFormat {
                attributes,
                attr_inspect_opts,
                target_call_stacks,
                package_values: None,
                writer: JsonWriter { json_lines: false },
            },
        }
    }

    pub fn format_report(
        &self,
        result: &ConfiguredTargetsWithErrors,
        target_hash_lookup: impl Fn(
            &ConfiguredTargetNode,
        ) -> buck2_error::Result<Option<ConfiguredTargetHash>>,
        output: &mut String,
        stderr: &mut String,
    ) -> buck2_error::Result<()> {
        output.push_str("{\n  \"compatible_targets\": [\n");

        // Format compatible targets
        let mut needs_separator = false;
        for node in &result.compatible_targets {
            if needs_separator {
                self.json_format.writer.separator(output);
            }
            needs_separator = true;
            ConfiguredTargetFormatter::target(
                &self.json_format,
                node,
                target_hash_lookup(node)?,
                output,
            )?;
        }

        output.push_str("\n  ],\n  \"incompatible_targets\": [\n");

        // Format incompatible targets
        let mut needs_separator = false;
        for reason in &result.incompatible_targets {
            if needs_separator {
                self.json_format.writer.separator(output);
            }
            needs_separator = true;
            self.format_incompatible_target(reason, output);
        }

        output.push_str("\n  ]\n}\n");

        // Errors only to stderr
        for error in &result.errors {
            let mut stderr_buf = String::new();
            if let Some(package) = &error.package {
                ConfiguredTargetFormatter::package_error(
                    &self.json_format,
                    package.package,
                    &error.error,
                    &mut String::new(),
                    &mut stderr_buf,
                );
            } else {
                ConfiguredTargetFormatter::target_error(
                    &self.json_format,
                    &error.error,
                    &mut String::new(),
                    &mut stderr_buf,
                );
            }
            stderr.push_str(&stderr_buf);
        }

        Ok(())
    }

    fn format_incompatible_target(
        &self,
        reason: &Arc<IncompatiblePlatformReason>,
        output: &mut String,
    ) {
        self.json_format.writer.entry_start(output);
        let mut is_first_entry = true;

        self.json_format.writer.entry_item(
            output,
            &mut is_first_entry,
            "buck.target",
            QuotedJson::quote_display(reason.target.unconfigured()),
        );

        let cause_str = match &reason.cause {
            IncompatiblePlatformReasonCause::UnsatisfiedConfig(_)
            | IncompatiblePlatformReasonCause::SelectIncompatible(_)
            | IncompatiblePlatformReasonCause::UnmatchedSelect { .. } => "ValidIncompatible",
            IncompatiblePlatformReasonCause::Dependency(_) => "InvalidIncompatible",
        };
        self.json_format.writer.entry_item(
            output,
            &mut is_first_entry,
            "buck.incompatible.cause",
            QuotedJson::quote_str(cause_str),
        );

        self.json_format.writer.entry_item(
            output,
            &mut is_first_entry,
            "buck.incompatible.reason",
            QuotedJson::quote_str(&format!("{:#}", reason)),
        );

        self.json_format.writer.entry_end(output, is_first_entry);
    }
}

pub(crate) fn create_formatter(
    request: &TargetsRequest,
    other: &targets_request::Other,
) -> buck2_error::Result<Arc<dyn TargetFormatter>> {
    let output_format = OutputFormat::try_from(request.output_format)
        .internal_error("Invalid value of `output_format`")?;

    let target_call_stacks = request.client_context()?.target_call_stacks;

    match output_format {
        OutputFormat::Json | OutputFormat::JsonLines => {}
        _ => {
            // Self-check.
            if !other.output_attributes.is_empty() {
                return Err(internal_error!(
                    "Attributes can only be specified when output format is JSON"
                ));
            }
        }
    }

    match output_format {
        OutputFormat::Unknown => Err(internal_error!("`output_format` is not set")),
        OutputFormat::Stats => Ok(Arc::new(StatsFormat)),
        OutputFormat::Text => Ok(Arc::new(TargetNameFormat {
            target_call_stacks,
            target_hash_graph_type: TargetHashGraphType::try_from(other.target_hash_graph_type)
                .expect("buck cli should send valid target hash graph type"),
        })),
        OutputFormat::Json | OutputFormat::JsonLines => Ok(Arc::new(JsonFormat {
            attributes: if other.output_attributes.is_empty() {
                None
            } else {
                Some(RegexSet::new(&other.output_attributes)?)
            },
            attr_inspect_opts: if other.include_default_attributes {
                AttrInspectOptions::All
            } else {
                AttrInspectOptions::DefinedOnly
            },
            target_call_stacks,
            package_values: if other.package_values.is_empty() {
                None
            } else {
                Some(RegexSet::new(&other.package_values)?)
            },
            writer: JsonWriter {
                json_lines: output_format == OutputFormat::JsonLines,
            },
        })),
    }
}

pub enum ConfiguredOutputHandler {
    Formatter(Arc<dyn ConfiguredTargetFormatter>),
    JsonReport(JsonReportFormat),
}

pub fn create_configured_formatter(
    request: &ConfiguredTargetsRequest,
) -> buck2_error::Result<ConfiguredOutputHandler> {
    let output_format = ConfiguredOutputFormat::try_from(request.output_format)?;
    let target_call_stacks = request.client_context()?.target_call_stacks;

    match output_format {
        ConfiguredOutputFormat::Text => Ok(ConfiguredOutputHandler::Formatter(Arc::new(
            TargetNameFormat {
                target_call_stacks,
                target_hash_graph_type: TargetHashGraphType::None,
            },
        ))),
        ConfiguredOutputFormat::Json => {
            Ok(ConfiguredOutputHandler::Formatter(Arc::new(JsonFormat {
                attributes: if request.output_attributes.is_empty() {
                    None
                } else {
                    Some(RegexSet::new(&request.output_attributes)?)
                },
                attr_inspect_opts: AttrInspectOptions::DefinedOnly,
                target_call_stacks,
                package_values: None,
                writer: JsonWriter { json_lines: false },
            })))
        }
        ConfiguredOutputFormat::JsonReport => {
            Ok(ConfiguredOutputHandler::JsonReport(JsonReportFormat::new(
                if request.output_attributes.is_empty() {
                    None
                } else {
                    Some(RegexSet::new(&request.output_attributes)?)
                },
                AttrInspectOptions::DefinedOnly,
                target_call_stacks,
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use buck2_error::ErrorTag;

    use super::Stats;

    /// `Stats::merge` carries the error tags of the merged stats, so the error built by the
    /// streaming command keeps the tags of the package failures.
    #[test]
    fn test_stats_merge_error_tags() {
        let e = buck2_error::buck2_error!(ErrorTag::Tier0, "package failed");
        let mut package = Stats::default();
        package.add_error(&e);
        assert!(
            package
                .to_error()
                .unwrap()
                .tags()
                .contains(&ErrorTag::Tier0)
        );

        let mut total = Stats::default();
        total.merge(&package);
        let err = total.to_error().unwrap();
        assert!(err.tags().contains(&ErrorTag::Tier0));
        assert!(err.tags().contains(&ErrorTag::Input));
    }
}
