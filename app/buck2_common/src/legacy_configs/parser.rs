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
use std::sync::Arc;
use std::sync::LazyLock;

use allocative::Allocative;
use buck2_core::cells::cell_root_path::CellRootPath;
use buck2_error::BuckErrorContext;
use buck2_fs::paths::RelativePath;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_hash::BuckFixedSortedMap;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::BoxFuture;
use itertools::Itertools;
use pagable::Pagable;
use regex::Regex;

use super::cells::ExternalPathBuckconfigData;
use crate::legacy_configs::args::ResolvedConfigFlag;
use crate::legacy_configs::configs::ConfigArgumentParseError;
use crate::legacy_configs::configs::ConfigData;
use crate::legacy_configs::configs::ConfigFileLocation;
use crate::legacy_configs::configs::ConfigFileLocationWithLine;
use crate::legacy_configs::configs::ConfigValue;
use crate::legacy_configs::configs::LegacyBuckConfig;
use crate::legacy_configs::configs::LegacyBuckConfigSection;
use crate::legacy_configs::configs::Location;
use crate::legacy_configs::file_ops::ConfigParserFileOps;
use crate::legacy_configs::file_ops::ConfigPath;
use crate::legacy_configs::key::BuckconfigKeyRef;
use crate::legacy_configs::parser::resolver::ConfigResolver;

mod resolver;

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum ConfigError {
    #[error("Expected line of the form `key = value` but key was empty. Line was `{0}`")]
    EmptyKey(String),
    #[error("Included file doesn't exist `{0}`")]
    MissingInclude(String),
    #[error("Improperly formatted section. Expected something of the form `[section]`, got {0}")]
    SectionMissingTrailingBracket(String),
    #[error("Improperly include directive path. Got {0}")]
    BadIncludePath(String),
    #[error("Detected a cycle in buckconfig includes: `{0}` is already being parsed")]
    IncludeCycle(String),
    #[error(
        "Couldn't parse line. Expected include directive (`<file:/file.bcfg>`), section(`[some_section]`), or key assignment (`some_key = some_value`). Got `{0}`"
    )]
    InvalidLine(String),
    #[error("Detected cycles in buckconfig $(config) references: {}", format_cycle(.0))]
    ReferenceCycle(Vec<(String, String)>),
}

fn format_cycle(cycle: &[(String, String)]) -> String {
    cycle
        .iter()
        .map(|(section, key)| format!("`{section}.{key}`"))
        .join(" -> ")
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Allocative, Pagable)]
struct SectionBuilder {
    values: BTreeMap<String, ConfigValue>,
}

impl SectionBuilder {
    fn finish(self) -> LegacyBuckConfigSection {
        LegacyBuckConfigSection {
            values: BuckFixedSortedMap::from_iter(self.values),
        }
    }
}

/// Represents the state associated with a buckconfig that is being parsed right now.
///
/// A buckconfig will generally be parsed by combining multiple command args and files
#[derive(Debug, Clone, PartialEq, Eq, Allocative, Pagable)]
pub(crate) struct LegacyConfigParser {
    values: BTreeMap<String, SectionBuilder>,
}

/// Represents the state associated with parsing a single file into a buckconfig.
struct LegacyConfigFileParser<'p> {
    include_stack: Vec<ConfigFileLocationWithLine>,
    current_file: Option<Arc<ConfigFileLocation>>,
    current_section: (String, BTreeMap<String, ConfigValue>),
    values: &'p mut LegacyConfigParser,
}

/// Matches file include directives. `optional` indicates whether it's an
/// optional include, `include` is the path.  Examples:
///   <file:/some/absolute>
///   <?file:/optional/absolute>
///   <file:relative/to/current>
///   <file:../../doesnt/need/to/be/forward/relative>
static FILE_INCLUDE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("<(?P<optional>\\?)?file:(?P<include>..*)>").unwrap());

impl LegacyConfigParser {
    pub(crate) fn new() -> Self {
        LegacyConfigParser {
            values: BTreeMap::new(),
        }
    }

    pub(crate) async fn parse_file(
        &mut self,
        path: &ConfigPath,
        source: Option<Location>,
        follow_includes: bool,
        file_ops: &mut dyn ConfigParserFileOps,
    ) -> buck2_error::Result<()> {
        let mut file_parser = LegacyConfigFileParser::new(self);
        file_parser.start_file(path, source)?;
        file_parser
            .parse_file_on_stack(path, follow_includes, file_ops)
            .await
            .with_buck_error_context(|| format!("Error parsing buckconfig `{path}`"))?;
        file_parser.finish_file();

        Ok(())
    }

    pub(crate) fn apply_config_arg(
        &mut self,
        config_pair: &ResolvedConfigFlag,
        current_cell: &CellRootPath,
    ) -> buck2_error::Result<()> {
        for banned_section in ["repositories", "cells"] {
            if config_pair.section == banned_section {
                return Err(
                    ConfigArgumentParseError::CellOverrideViaCliConfig(banned_section).into(),
                );
            };
        }
        let pair = config_pair.to_owned();
        let cell_matches = pair.cell.as_deref() == Some(current_cell) || pair.cell.is_none();
        if cell_matches {
            let config_section = self.values.entry(pair.section).or_default();

            match pair.value {
                Some(raw_value) => {
                    let config_value = ConfigValue::new_raw_arg(raw_value);
                    config_section.values.insert(pair.key, config_value)
                }
                None => config_section.values.remove(&pair.key),
            };
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> buck2_error::Result<LegacyBuckConfig> {
        let LegacyConfigParser { values } = self;

        let values = ConfigResolver::resolve(values)?;

        Ok(LegacyBuckConfig(Arc::new(ConfigData { values })))
    }

    pub(crate) fn join(&mut self, other: &LegacyConfigParser) {
        for (section, section_builder) in other.values.iter() {
            for (key, value) in section_builder.values.iter() {
                self.values
                    .entry(section.to_owned())
                    .or_default()
                    .values
                    .insert(key.to_owned(), value.clone());
            }
        }
    }

    pub(crate) fn filter_values<F>(mut self, filter: F) -> Self
    where
        F: Fn(&BuckconfigKeyRef) -> bool,
    {
        for (section, section_builder) in self.values.iter_mut() {
            section_builder.values.retain(|key, _| {
                filter(&BuckconfigKeyRef {
                    section,
                    property: key,
                })
            });
        }
        self
    }

    pub(crate) fn to_proto_external_config_values(
        &self,
        is_cli: bool,
    ) -> Vec<buck2_data::ConfigValue> {
        self.values
            .iter()
            .flat_map(|(k, v)| {
                v.values.iter().map(|(key, value)| buck2_data::ConfigValue {
                    section: k.to_owned(),
                    key: key.to_owned(),
                    value: value.raw_value().to_owned(),
                    cell: None,
                    is_cli,
                })
            })
            .collect()
    }
    pub(crate) fn combine(external_path_configs: Vec<ExternalPathBuckconfigData>) -> Self {
        let mut parser = LegacyConfigParser::new();
        external_path_configs
            .into_iter()
            .for_each(|o| parser.join(&o.parse_state));
        parser
    }
}

impl<'p> LegacyConfigFileParser<'p> {
    fn new(values: &'p mut LegacyConfigParser) -> Self {
        LegacyConfigFileParser {
            include_stack: Vec::new(),
            current_file: None,
            current_section: Self::unspecified_section(),
            values,
        }
    }

    fn unspecified_section() -> (String, BTreeMap<String, ConfigValue>) {
        ("__unspecified__".to_owned(), BTreeMap::new())
    }

    fn push_file(&mut self, line: usize, path: &ConfigPath) -> buck2_error::Result<()> {
        let current_file = self.current_file.dupe().unwrap_or_else(|| panic!("push_file() called without any files on the include stack. top-level files should use start_file()"));
        let path_string = path.to_string();
        if current_file.path == path_string
            || self
                .include_stack
                .iter()
                .any(|included_from| included_from.source_file.path == path_string)
        {
            return Err(ConfigError::IncludeCycle(path_string).into());
        }
        let include_source = ConfigFileLocationWithLine {
            source_file: current_file,
            line,
        };

        self.include_stack.push(include_source.clone());

        let source_file = Arc::new(ConfigFileLocation {
            path: path.to_string(),
            include_source: Some(Location::File(include_source)),
        });
        self.current_file = Some(source_file);
        Ok(())
    }

    fn start_file(
        &mut self,
        path: &ConfigPath,
        source: Option<Location>,
    ) -> buck2_error::Result<()> {
        let source_file = Arc::new(ConfigFileLocation {
            path: path.to_string(),
            include_source: source,
        });
        self.current_file = Some(source_file);
        Ok(())
    }

    fn pop_file(&mut self) {
        match self.include_stack.pop() {
            Some(loc) => {
                self.current_file = Some(loc.source_file);
            }
            None => {
                self.current_file = None;
            }
        }
    }

    fn location(&self, line_number: usize) -> ConfigFileLocationWithLine {
        ConfigFileLocationWithLine {
            source_file: self
                .current_file
                .dupe()
                .unwrap_or_else(|| panic!("tried to get location without any current file.")),
            // Our line numbers at this point are 0-based, but most people expect file line numbers to be 1-based.
            line: line_number + 1,
        }
    }

    /// Return value indicates whether the file existed or not
    fn parse_file_on_stack<'a>(
        &'a mut self,
        config_path: &'a ConfigPath,
        parse_includes: bool,
        file_ops: &'a mut dyn ConfigParserFileOps,
    ) -> BoxFuture<'a, buck2_error::Result<bool>> {
        async move {
            let Some(file_lines) = file_ops.read_file_lines_if_exists(config_path).await? else {
                return Ok(false);
            };
            self.parse_lines(config_path, file_lines, parse_includes, file_ops)
                .await?;
            Ok(true)
        }
        .boxed()
    }

    fn is_comment_line(line: &str) -> bool {
        line.starts_with('#') || line.starts_with(';')
    }

    fn strip_line_comment(line: &str) -> &str {
        match line.split_once(" #") {
            Some((before, _)) => before,
            None => line,
        }
    }

    fn parse_section_marker(line: &str) -> buck2_error::Result<Option<&str>> {
        // We allow trailing comment markers at the end of sections, since otherwise
        // using oss-enable/oss-disable is super tricky
        match line.strip_prefix('[') {
            Some(remaining) => match Self::strip_line_comment(remaining).strip_suffix(']') {
                None => Err(ConfigError::SectionMissingTrailingBracket(line.to_owned()).into()),
                Some(section) => Ok(Some(section)),
            },
            None => Ok(None),
        }
    }

    async fn parse_lines(
        &mut self,
        config_path: &ConfigPath,
        lines: Vec<String>,
        parse_includes: bool,
        file_ops: &mut dyn ConfigParserFileOps,
    ) -> buck2_error::Result<()> {
        let lines = lines
            .into_iter()
            // Trim leading/trailing whitespace.
            .map(|line| line.trim().to_owned())
            // add line numbers
            .enumerate()
            // Coalesce escaped newlines.
            .coalesce(|(i, mut prev), (j, next)| {
                if prev.ends_with('\\') && !Self::is_comment_line(&prev) {
                    prev.truncate(prev.len() - 1);
                    if !Self::is_comment_line(&next) {
                        prev.push_str(&next);
                    }
                    Ok((i, prev))
                } else {
                    Err(((i, prev), (j, next)))
                }
            })
            // Remove commented lines.
            // This needs to come after the coalesce in case someone has an empty line after an escaped newline
            // Remove empty lines and comment lines (support both '#' and ';' for comment lines)
            .filter(|(_, l)| !l.is_empty() && !Self::is_comment_line(l));

        for (i, line) in lines {
            if let Some(section) = Self::parse_section_marker(&line)? {
                // Start the new section, grabbing the recorded values for the previous
                // section.
                let section = std::mem::replace(
                    &mut self.current_section,
                    (section.to_owned(), BTreeMap::new()),
                );
                self.commit_section(section)
            } else if let Some((key, val)) = line.split_once('=') {
                let key = key.trim();
                let val = val.trim();
                if key.is_empty() {
                    return Err(ConfigError::EmptyKey(line.to_owned()).into());
                }
                self.current_section.1.insert(
                    key.to_owned(),
                    ConfigValue::new_raw(self.location(i), val.to_owned()),
                );
            } else if let Some(m) = FILE_INCLUDE.captures(&line) {
                if parse_includes {
                    let include = m.name("include").unwrap().as_str();
                    let include = if cfg!(windows) && include.contains(':') {
                        // On Windows absolute includes look like /C:/foo/bar.
                        // For compatibility with Python parser we need to support this.
                        include.trim_start_matches('/')
                    } else {
                        include
                    };
                    let optional = m.name("optional").is_some();
                    // Note: Using `AbsNormPath` to preserve existing behavior of requiring normalized paths
                    let include_file = if let Ok(absolute) = AbsNormPath::new(include) {
                        ConfigPath::Global(absolute.to_owned().into_abs_path_buf())
                    } else {
                        let relative = RelativePath::unchecked_new(include);
                        match config_path.join_to_parent_normalized(relative) {
                            Ok(d) => d,
                            Err(_) => {
                                return Err(ConfigError::BadIncludePath(include.to_owned()).into());
                            }
                        }
                    };

                    self.push_file(i, &include_file)?;
                    let exists = self
                        .parse_file_on_stack(&include_file, parse_includes, file_ops)
                        .await?;
                    self.pop_file();

                    if !exists && !optional {
                        return Err(ConfigError::MissingInclude(include.to_owned()).into());
                    }
                }
            } else {
                return Err(ConfigError::InvalidLine(line.to_owned()).into());
            }
        }
        Ok(())
    }

    fn commit_section(&mut self, section: (String, BTreeMap<String, ConfigValue>)) {
        let (section, values) = section;
        // Commit the previous section.
        let committed = self.values.values.entry(section).or_default();
        values.into_iter().for_each(|(k, v)| {
            committed.values.insert(k, v);
        });
    }

    fn finish_file(&mut self) {
        self.pop_file();

        let section = std::mem::replace(&mut self.current_section, Self::unspecified_section());
        self.commit_section(section);
    }
}
