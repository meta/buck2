/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_cli_proto::ConfigOverride;
use buck2_cli_proto::config_override::ConfigType;
use buck2_core::cells::cell_root_path::CellRootPath;
use buck2_core::cells::cell_root_path::CellRootPathBuf;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorContext;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::abs_path::AbsPathBuf;
use pagable::Pagable;

use crate::legacy_configs::configs::ConfigArgumentParseError;
use crate::legacy_configs::configs::ConfigSectionAndKey;
use crate::legacy_configs::configs::LegacyBuckConfig;
use crate::legacy_configs::configs::parse_config_section_and_key;
use crate::legacy_configs::file_ops::ConfigParserFileOps;
use crate::legacy_configs::file_ops::ConfigPath;
use crate::legacy_configs::parser::LegacyConfigParser;

/// The cell-aware constructors and accessor for `ConfigOverride`. They live here rather
/// than on the type because `buck2_cli_proto` deliberately does not depend on `buck2_core`.
pub trait ConfigOverrideExt: Sized {
    fn flag(s: &str, cell: Option<CellRootPathBuf>) -> Self;

    fn file(p: &str, cell: Option<CellRootPathBuf>) -> Self;

    fn get_cell(&self) -> buck2_error::Result<Option<&CellRootPath>>;
}

impl ConfigOverrideExt for ConfigOverride {
    fn flag(s: &str, cell: Option<CellRootPathBuf>) -> Self {
        Self {
            cell: cell.map(|c| c.as_str().to_owned()),
            config_override: s.to_owned(),
            config_type: ConfigType::Value.into(),
        }
    }

    fn file(p: &str, cell: Option<CellRootPathBuf>) -> Self {
        Self {
            cell: cell.map(|c| c.as_str().to_owned()),
            config_override: p.to_owned(),
            config_type: ConfigType::File.into(),
        }
    }

    fn get_cell(&self) -> buck2_error::Result<Option<&CellRootPath>> {
        self.cell
            .as_ref()
            .map(|p| {
                ProjectRelativePath::new(p)
                    .map(CellRootPath::new)
                    .internal_error("Client should have sent a valid path")
            })
            .transpose()
    }
}

/// Representation of a processed config arg, namely after file path resolution has been performed.
#[derive(Debug, Clone, PartialEq, Eq, allocative::Allocative, Pagable)]
pub(crate) enum ResolvedLegacyConfigArg {
    /// A single config key-value pair (in `a.b=c` format).
    Flag(ResolvedConfigFlag),
    /// A file containing additional config values (in `.buckconfig` format).
    File(ResolvedConfigFile),
}

#[derive(Clone, Debug, PartialEq, Eq, allocative::Allocative, Pagable)]
pub(crate) enum ResolvedConfigFile {
    /// If the config file is project relative, the path of the file
    Project(ProjectRelativePathBuf),
    /// If the config file is external, we pre-parse it to be able to insert the results into dice
    Global(ExternalConfigFile),
}

#[derive(Clone, Debug, PartialEq, Eq, allocative::Allocative, Pagable)]
pub(crate) struct ExternalConfigFile {
    pub(crate) parser: LegacyConfigParser,
    // The origin path of the config file
    origin_path: AbsPathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, allocative::Allocative, Pagable)]
pub(crate) struct ResolvedConfigFlag {
    pub(crate) section: String,
    pub(crate) key: String,
    // None value means this config is unset.
    pub(crate) value: Option<String>,
    // If this arg only applies to one cell, the root of that cell.
    pub(crate) cell: Option<CellRootPathBuf>,
}

/// Renders the arg close to how it was passed on the command line, for user-facing messages.
/// A cell-scoped flag shows the cell's root path rather than the alias the user typed, since
/// the alias is not retained after resolution.
impl std::fmt::Display for ResolvedLegacyConfigArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Flag(flag) => {
                write!(f, "--config ")?;
                if let Some(cell) = &flag.cell {
                    write!(f, "{cell}//")?;
                }
                write!(
                    f,
                    "{}.{}={}",
                    flag.section,
                    flag.key,
                    flag.value.as_deref().unwrap_or("")
                )
            }
            Self::File(ResolvedConfigFile::Project(path)) => write!(f, "--config-file {path}"),
            Self::File(ResolvedConfigFile::Global(file)) => {
                write!(f, "--config-file {}", file.origin_path)
            }
        }
    }
}

fn resolve_config_flag_arg(
    cell: Option<CellRootPathBuf>,
    raw_arg: &str,
) -> buck2_error::Result<ResolvedConfigFlag> {
    let (raw_section_and_key, raw_value) = raw_arg
        .split_once('=')
        .ok_or_else(|| ConfigArgumentParseError::NoEqualsSeparator(raw_arg.to_owned()))?;
    let ConfigSectionAndKey { section, key } =
        parse_config_section_and_key(raw_section_and_key, Some(raw_arg))?;

    let value = match raw_value {
        "" => None, // An empty string unsets this config.
        v => Some(v.to_owned()),
    };

    Ok(ResolvedConfigFlag {
        section,
        key,
        value,
        cell,
    })
}

async fn resolve_config_file_arg(
    cell: Option<CellRootPathBuf>,
    arg: &str,
    file_ops: &mut dyn ConfigParserFileOps,
) -> buck2_error::Result<ResolvedConfigFile> {
    if let Some(cell_path) = cell {
        let proj_path = cell_path.as_project_relative_path().join_normalized(arg)?;
        return Ok(ResolvedConfigFile::Project(proj_path));
    }

    let path = AbsPath::new(arg).internal_error("Client always produces absolute paths")?;
    Ok(ResolvedConfigFile::Global(ExternalConfigFile {
        origin_path: AbsPathBuf::new(arg)?,
        parser: LegacyConfigParser::combine(
            LegacyBuckConfig::start_parse_for_external_files(
                &[ConfigPath::Global(path.to_owned())],
                file_ops,
                // Note that when reading immediate configs that don't follow includes, we don't apply
                // config args either
                true, // follow includes
            )
            .await?,
        ),
    }))
}

pub(crate) async fn resolve_config_args(
    args: &[ConfigOverride],
    file_ops: &mut dyn ConfigParserFileOps,
) -> buck2_error::Result<Vec<ResolvedLegacyConfigArg>> {
    let mut resolved_args = Vec::new();

    for u in args {
        let config_type = ConfigType::try_from(u.config_type).with_buck_error_context(|| {
            format!(
                "Unknown ConfigType enum value `{}` when trying to deserialize",
                u.config_type
            )
        })?;
        let resolved = match config_type {
            ConfigType::Value => {
                let cell = u.get_cell()?.map(|p| p.to_buf());
                let resolved_flag = resolve_config_flag_arg(cell, &u.config_override)?;
                ResolvedLegacyConfigArg::Flag(resolved_flag)
            }
            ConfigType::File => {
                let cell = u.get_cell()?.map(|p| p.to_buf());
                let resolved_path =
                    resolve_config_file_arg(cell, &u.config_override, file_ops).await?;
                ResolvedLegacyConfigArg::File(resolved_path)
            }
        };
        resolved_args.push(resolved);
    }

    Ok(resolved_args)
}

pub(crate) fn to_proto_config_args(
    args: &[ResolvedLegacyConfigArg],
) -> Vec<buck2_data::BuckconfigComponent> {
    use buck2_data::buckconfig_component::Data::ConfigFile;
    use buck2_data::buckconfig_component::Data::ConfigValue;
    use buck2_data::config_file::Data::GlobalExternalConfig;
    use buck2_data::config_file::Data::ProjectRelativePath;

    args.iter()
        .map(|arg| {
            let data = match arg {
                ResolvedLegacyConfigArg::Flag(resolved_config_flag) => {
                    ConfigValue(buck2_data::ConfigValue {
                        section: resolved_config_flag.section.to_owned(),
                        key: resolved_config_flag.key.to_owned(),
                        value: resolved_config_flag
                            .value
                            .clone()
                            .unwrap_or("not_set".to_owned()),
                        cell: resolved_config_flag
                            .cell
                            .clone()
                            .map(|flag| flag.to_string()),
                        is_cli: true,
                    })
                }
                ResolvedLegacyConfigArg::File(ResolvedConfigFile::Project(p)) => {
                    ConfigFile(buck2_data::ConfigFile {
                        data: Some(ProjectRelativePath(p.to_string())),
                    })
                }
                ResolvedLegacyConfigArg::File(ResolvedConfigFile::Global(p)) => {
                    ConfigFile(buck2_data::ConfigFile {
                        data: Some(GlobalExternalConfig(buck2_data::GlobalExternalConfig {
                            values: p.parser.to_proto_external_config_values(true),
                            origin_path: p.origin_path.to_str().unwrap_or("").to_owned(),
                        })),
                    })
                }
            };
            buck2_data::BuckconfigComponent { data: Some(data) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::resolve_config_flag_arg;

    #[test]
    fn test_argument_pair() -> buck2_error::Result<()> {
        // Valid Formats

        let normal_pair = resolve_config_flag_arg(None, "apple.key=value")?;

        assert_eq!("apple", normal_pair.section);
        assert_eq!("key", normal_pair.key);
        assert_eq!(Some("value".to_owned()), normal_pair.value);

        let unset_pair = resolve_config_flag_arg(None, "apple.key=")?;

        assert_eq!("apple", unset_pair.section);
        assert_eq!("key", unset_pair.key);
        assert_eq!(None, unset_pair.value);

        // Whitespace

        let section_leading_whitespace = resolve_config_flag_arg(None, "  apple.key=value")?;
        assert_eq!("apple", section_leading_whitespace.section);
        assert_eq!("key", section_leading_whitespace.key);
        assert_eq!(Some("value".to_owned()), section_leading_whitespace.value);

        let pair_with_whitespace_in_key = resolve_config_flag_arg(None, "apple. key=value");
        assert!(pair_with_whitespace_in_key.is_err());

        let pair_with_whitespace_in_value =
            resolve_config_flag_arg(None, "apple.key= value with whitespace  ")?;
        assert_eq!("apple", pair_with_whitespace_in_value.section);
        assert_eq!("key", pair_with_whitespace_in_value.key);
        assert_eq!(
            Some(" value with whitespace  ".to_owned()),
            pair_with_whitespace_in_value.value
        );

        // Invalid Formats

        let pair_without_section = resolve_config_flag_arg(None, "key=value");
        assert!(pair_without_section.is_err());

        let pair_without_equals = resolve_config_flag_arg(None, "apple.keyvalue");
        assert!(pair_without_equals.is_err());

        let pair_without_section_or_equals = resolve_config_flag_arg(None, "applekeyvalue");
        assert!(pair_without_section_or_equals.is_err());

        Ok(())
    }
}
