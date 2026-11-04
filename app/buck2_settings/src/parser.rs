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
use std::path::PathBuf;
use std::sync::Arc;

use buck2_core::buck2_env;
use buck2_core::fs::project::ProjectRoot;
#[cfg(fbcode_build)]
use buck2_core::soft_error_or_panic;
use buck2_error::BuckErrorContext;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::abs_path::AbsPathBuf;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::BuckSettings;
use crate::metadata::OverrideSource;
use crate::metadata::SectionMetadata;
use crate::metadata::SettingKeyRef;
use crate::metadata::SettingSource;
use crate::metadata::find_setting_metadata;
use crate::path::DEFAULT_SETTINGS_SOURCES;
use crate::path::DOT_BUCKSETTINGS;
use crate::path::SettingsSource as SettingsPathSource;
#[cfg(fbcode_build)]
use crate::path::rollouts_path;
#[cfg(fbcode_build)]
use crate::rollouts::SelectedRollout;
#[cfg(fbcode_build)]
use crate::rollouts::select_rollout_table;
use crate::settings::ALL_SECTION_METADATA;
use crate::settings::BuckSettingsData;

/// Where a setting's value came from. A key no layer set has no origin.
#[derive(Debug, Clone, PartialEq, Eq, derive_more::Display, Serialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum SettingOrigin {
    /// The repository's `.bucksettings.toml`.
    #[display("repository settings file `{_0}`")]
    Repository(AbsPathBuf),
    /// A `.bucksettings.local.toml`, or the `BUCK2_SETTINGS_OVERRIDE` file.
    #[display("local settings file `{_0}`")]
    Local(AbsPathBuf),
    /// The wrapper's rollout cache.
    #[display("rollout cache `{_0}`")]
    Rollout(AbsPathBuf),
    #[display("`--setting`")]
    CommandLine,
    /// The buckconfig key a setting declares as its fallback. An origin in
    /// reports only, never a layer.
    #[display("buckconfig `{_0}`")]
    Buckconfig(String),
}

impl SettingOrigin {
    fn setting_source(&self) -> SettingSource {
        match self {
            Self::Repository(_) => SettingSource::Base,
            Self::Rollout(_) => SettingSource::Rollout,
            Self::Local(_) => SettingSource::Override(OverrideSource::LocalSettings),
            Self::CommandLine => SettingSource::Override(OverrideSource::CommandLine),
            // Never merged as a layer, so never checked against a policy.
            Self::Buckconfig(_) => SettingSource::Base,
        }
    }
}

/// A settings file or flag the parser considered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsLayerReport {
    pub origin: SettingOrigin,
    /// Whether the layer contributed: a file that exists and parsed, a flag.
    pub applied: bool,
    /// What the layer held but did not contribute. Only the rollout cache
    /// tolerates that; any other layer fails the command instead.
    pub ignored: Vec<IgnoredSetting>,
}

/// Rollout content that was not applied, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IgnoredSetting {
    /// `section.key`, or `section` when a whole section was skipped; `None`
    /// when the whole file was.
    pub setting: Option<String>,
    pub reason: String,
}

impl IgnoredSetting {
    #[cfg(fbcode_build)]
    pub(crate) fn key(section: &str, key: &str, reason: impl Into<String>) -> Self {
        Self {
            setting: Some(format!("{section}.{key}")),
            reason: reason.into(),
        }
    }

    #[cfg(fbcode_build)]
    pub(crate) fn section(section: &str, reason: impl Into<String>) -> Self {
        Self {
            setting: Some(section.to_owned()),
            reason: reason.into(),
        }
    }

    #[cfg(fbcode_build)]
    fn layer(reason: impl Into<String>) -> Self {
        Self {
            setting: None,
            reason: reason.into(),
        }
    }
}

/// Settings together with the layers they came from.
#[derive(Debug)]
pub struct ResolvedSettings {
    pub settings: BuckSettings,
    /// Lowest priority first.
    pub layers: Vec<SettingsLayerReport>,
    origins: BTreeMap<(String, String), Arc<SettingOrigin>>,
}

impl ResolvedSettings {
    /// Where the winning value of `section.key` came from; `None` when no
    /// layer set it.
    pub fn origin(&self, section: &str, key: &str) -> Option<&SettingOrigin> {
        self.origins
            .get(&(section.to_owned(), key.to_owned()))
            .map(Arc::as_ref)
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum SettingsError {
    #[error("Error parsing buck settings: {0}")]
    Parse(toml::de::Error),
    #[error("BUCK2_SETTINGS_OVERRIDE file `{0}` does not exist")]
    OverrideFileMissing(String),
    #[error("Buck setting `{key}` cannot be overridden from {origin}")]
    InvalidOverride {
        key: String,
        origin: Arc<SettingOrigin>,
    },
}

/// Settings parsed from a single source.
#[derive(Clone, Debug)]
struct SettingsLayer {
    provenance: Arc<SettingOrigin>,
    table: toml::Table,
}

/// The rollout layer and what was left out of it.
#[derive(Default)]
struct RolloutLayer {
    layer: Option<SettingsLayer>,
    ignored: Vec<IgnoredSetting>,
}

#[cfg(fbcode_build)]
struct LoadedRollout {
    path: AbsPathBuf,
    selected: SelectedRollout,
}

impl SettingsLayer {
    fn new(origin: SettingOrigin, table: toml::Table) -> Self {
        Self {
            provenance: Arc::new(origin),
            table,
        }
    }

    fn setting_flag(table: toml::Table) -> Self {
        Self::new(SettingOrigin::CommandLine, table)
    }
}

/// A node in `MergedSettings`. Either a section or a setting value with provenance.
#[derive(Debug)]
enum SourcedValue {
    /// A settings section containing merged values.
    Section(MergedSettings),
    /// A setting value from one source, replaced by higher-priority sources during merging.
    Leaf {
        value: toml::Value,
        provenance: Arc<SettingOrigin>,
    },
}

impl SourcedValue {
    fn new(value: toml::Value, provenance: &Arc<SettingOrigin>) -> Self {
        match value {
            toml::Value::Table(table) => Self::Section(MergedSettings(
                table
                    .into_iter()
                    .map(|(key, value)| (key, Self::new(value, provenance)))
                    .collect(),
            )),
            value => Self::Leaf {
                value,
                provenance: Arc::clone(provenance),
            },
        }
    }

    fn into_value(self) -> toml::Value {
        match self {
            Self::Section(section) => toml::Value::Table(section.into_table()),
            Self::Leaf { value, .. } => value,
        }
    }
}

/// Recursively merged settings keyed by section or setting name at each level.
#[derive(Debug, Default)]
struct MergedSettings(BTreeMap<String, SourcedValue>);

impl MergedSettings {
    fn into_table(self) -> toml::Table {
        self.0
            .into_iter()
            .map(|(key, value)| (key, value.into_value()))
            .collect()
    }

    fn validate_inner<'a>(
        &'a self,
        sections: &[SectionMetadata],
        path: &mut Vec<&'a str>,
    ) -> buck2_error::Result<()> {
        for (name, value) in &self.0 {
            path.push(name);
            match value {
                SourcedValue::Section(section) => {
                    section.validate_inner(sections, path)?;
                }
                SourcedValue::Leaf { provenance, .. } => {
                    let (name, section) = path.split_last().expect("a setting always has a name");
                    if section.is_empty() {
                        path.pop();
                        continue;
                    }
                    let section = section.join(".");
                    let key = SettingKeyRef {
                        section: &section,
                        name,
                    };
                    let Some(metadata) = find_setting_metadata(sections, key) else {
                        path.pop();
                        continue;
                    };
                    if !metadata.allows_source(provenance.setting_source()) {
                        return Err(SettingsError::InvalidOverride {
                            key: path.join("."),
                            origin: Arc::clone(provenance),
                        }
                        .into());
                    }
                }
            }
            path.pop();
        }
        Ok(())
    }

    /// Validates override sources for final merged values, ignoring overridden values.
    fn validate(&self, sections: &[SectionMetadata]) -> buck2_error::Result<()> {
        self.validate_inner(sections, &mut Vec::new())
    }

    /// The origin of every `section.key` leaf.
    fn origins(&self) -> BTreeMap<(String, String), Arc<SettingOrigin>> {
        let mut origins = BTreeMap::new();
        for (section_name, section) in &self.0 {
            let SourcedValue::Section(section) = section else {
                continue;
            };
            for (name, value) in &section.0 {
                if let SourcedValue::Leaf { provenance, .. } = value {
                    origins.insert((section_name.clone(), name.clone()), Arc::clone(provenance));
                }
            }
        }
        origins
    }

    fn deserialize<T: DeserializeOwned>(self) -> buck2_error::Result<T> {
        Ok(toml::Value::Table(self.into_table())
            .try_into()
            .map_err(SettingsError::Parse)?)
    }
}

/// Parses a settings file into an untyped table.
fn parse_table(path: &AbsPath) -> buck2_error::Result<Option<toml::Table>> {
    let Some(content) = fs_util::read_to_string_if_exists(path)
        .with_buck_error_context(|| format!("Reading `{}`", path.display()))?
    else {
        return Ok(None);
    };

    let table = toml::from_str::<toml::Table>(&content)
        .map_err(SettingsError::Parse)
        .with_buck_error_context(|| format!("Parsing `{}`", path.display()))?;

    Ok(Some(table))
}

/// Parses settings files from lowest to highest priority.
/// The on-disk layers, lowest priority first, and a report of every file
/// considered, in the same order.
fn parse_layers(
    repo_root: &AbsPath,
    home_dir: Option<&AbsPath>,
    rollout: RolloutLayer,
) -> buck2_error::Result<(Vec<SettingsLayer>, Vec<SettingsLayerReport>)> {
    let mut base_layers = Vec::new();
    let mut base_reports = Vec::new();
    let mut local_layers = Vec::new();
    let mut local_reports = Vec::new();
    for source in DEFAULT_SETTINGS_SOURCES {
        let (path, is_base) = match source {
            SettingsPathSource::RepoRootFile(name) => {
                let path = repo_root.join(name);
                (path, *name == DOT_BUCKSETTINGS)
            }
            SettingsPathSource::HomeFile(name) => {
                let Some(home_dir) = home_dir else {
                    continue;
                };
                (home_dir.join(name), false)
            }
        };
        let table = parse_table(&path)?;
        let (origin, layers, reports) = if is_base {
            (
                SettingOrigin::Repository(path),
                &mut base_layers,
                &mut base_reports,
            )
        } else {
            (
                SettingOrigin::Local(path),
                &mut local_layers,
                &mut local_reports,
            )
        };
        reports.push(SettingsLayerReport {
            origin: origin.clone(),
            applied: table.is_some(),
            ignored: Vec::new(),
        });
        if let Some(table) = table {
            layers.push(SettingsLayer::new(origin, table));
        }
    }

    if let Some(home_dir) = home_dir
        && let Some(origin) = rollout_origin(home_dir)
    {
        base_reports.push(SettingsLayerReport {
            origin,
            applied: rollout.layer.is_some(),
            ignored: rollout.ignored,
        });
    }
    base_layers.extend(rollout.layer);
    base_layers.extend(local_layers);
    base_reports.extend(local_reports);
    Ok((base_layers, base_reports))
}

#[cfg(fbcode_build)]
fn rollout_origin(home_dir: &AbsPath) -> Option<SettingOrigin> {
    Some(SettingOrigin::Rollout(rollouts_path(home_dir)))
}

#[cfg(not(fbcode_build))]
fn rollout_origin(_home_dir: &AbsPath) -> Option<SettingOrigin> {
    None
}

#[cfg(fbcode_build)]
fn try_load_rollout(home_dir: Option<&AbsPath>) -> buck2_error::Result<Option<LoadedRollout>> {
    let Some(home_dir) = home_dir else {
        return Ok(None);
    };
    let path = rollouts_path(home_dir);
    let Some(versioned_settings) = parse_table(&path)? else {
        return Ok(None);
    };
    Ok(Some(LoadedRollout {
        path,
        selected: select_rollout_table(versioned_settings),
    }))
}

/// Loads the rollout layer. Nothing in the cache fails the command: a file
/// that cannot be read or parsed is skipped whole, and a stanza's unusable
/// keys are skipped one by one. Both are reported and logged, not shown: the
/// user cannot do anything about the wrapper's cache.
#[cfg(fbcode_build)]
fn rollout_layer(home_dir: Option<&AbsPath>) -> RolloutLayer {
    let LoadedRollout { path, selected } = match try_load_rollout(home_dir) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => return RolloutLayer::default(),
        Err(error) => {
            let reason = format!("{error:#}");
            soft_error_or_panic!(
                "buck_settings_rollout_ignored",
                error.context("Ignoring the settings rollout cache")
            );
            return RolloutLayer {
                layer: None,
                ignored: vec![IgnoredSetting::layer(reason)],
            };
        }
    };
    if !selected.ignored.is_empty() {
        let ignored: Vec<String> = selected
            .ignored
            .iter()
            .map(|ignored| {
                format!(
                    "{}: {}",
                    ignored.setting.as_deref().unwrap_or("file"),
                    ignored.reason
                )
            })
            .collect();
        soft_error_or_panic!(
            "buck_settings_rollout_ignored",
            buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "Ignored parts of the settings rollout cache `{path}`: [{}]",
                ignored.join(", ")
            )
        );
    }
    let layer = (!selected.layer.is_empty())
        .then(|| SettingsLayer::new(SettingOrigin::Rollout(path), selected.layer));
    RolloutLayer {
        layer,
        ignored: selected.ignored,
    }
}

#[cfg(not(fbcode_build))]
fn rollout_layer(_home_dir: Option<&AbsPath>) -> RolloutLayer {
    RolloutLayer::default()
}

/// Recursively merges overlay into base. Overlay values take precedence over base values.
/// TOML tables are merged recursively by key instead of replaced atomically.
fn merge(base: &mut MergedSettings, overlay: toml::Table, provenance: &Arc<SettingOrigin>) {
    for (k, v) in overlay {
        match (base.0.get_mut(&k), v) {
            (Some(SourcedValue::Section(base_section)), toml::Value::Table(overlay_table)) => {
                merge(base_section, overlay_table, provenance);
            }
            (_, v) => {
                base.0.insert(k, SourcedValue::new(v, provenance));
            }
        }
    }
}

fn merge_layers(layers: Vec<SettingsLayer>) -> MergedSettings {
    let mut merged = MergedSettings::default();
    for SettingsLayer { provenance, table } in layers {
        merge(&mut merged, table, &provenance);
    }
    merged
}

fn resolve_layers(
    layers: Vec<SettingsLayer>,
    reports: Vec<SettingsLayerReport>,
) -> buck2_error::Result<ResolvedSettings> {
    let merged = merge_layers(layers);
    merged.validate(ALL_SECTION_METADATA)?;
    let origins = merged.origins();
    let data: BuckSettingsData = merged.deserialize()?;
    Ok(ResolvedSettings {
        settings: data.into(),
        layers: reports,
        origins,
    })
}

fn resolve(layers: Vec<SettingsLayer>) -> buck2_error::Result<BuckSettings> {
    Ok(resolve_layers(layers, Vec::new())?.settings)
}

/// Resolves `tables` as if each were a `--setting` layer. For tests.
pub fn resolve_setting_flags(tables: Vec<toml::Table>) -> buck2_error::Result<BuckSettings> {
    resolve(
        tables
            .into_iter()
            .map(SettingsLayer::setting_flag)
            .collect(),
    )
}

pub fn parse_settings(
    project_fs: &ProjectRoot,
    settings_args: &[toml::Table],
) -> buck2_error::Result<BuckSettings> {
    Ok(resolve_settings(project_fs, settings_args)?.settings)
}

/// Like [`parse_settings`], keeping where each value came from.
pub fn resolve_settings(
    project_fs: &ProjectRoot,
    settings_args: &[toml::Table],
) -> buck2_error::Result<ResolvedSettings> {
    let repo_root = project_fs.root().as_abs_path();
    let home_dir = buck2_env!("BUCK2_TEST_SETTINGS_HOME_DIR", applicability = testing)?
        .map(PathBuf::from)
        .or_else(dirs::home_dir);
    let home_dir = home_dir.map(AbsPathBuf::new).transpose()?;
    let override_file = buck2_env!("BUCK2_SETTINGS_OVERRIDE", applicability = testing)?
        .map(|path| {
            // Absolute only: this parses in the daemon, whose cwd is not the
            // caller's, so a relative path has nothing sound to resolve
            // against.
            AbsPathBuf::new(PathBuf::from(path))
                .buck_error_context("`BUCK2_SETTINGS_OVERRIDE` must be an absolute path")
        })
        .transpose()?;
    resolve_settings_with_home(
        repo_root,
        home_dir.as_deref(),
        override_file.as_deref(),
        settings_args,
    )
}

#[cfg(test)]
fn parse_settings_with_home(
    repo_root: &AbsPath,
    home_dir: Option<&AbsPath>,
    override_file: Option<&AbsPath>,
    settings_args: &[toml::Table],
) -> buck2_error::Result<BuckSettings> {
    Ok(resolve_settings_with_home(repo_root, home_dir, override_file, settings_args)?.settings)
}

pub(crate) fn resolve_settings_with_home(
    repo_root: &AbsPath,
    home_dir: Option<&AbsPath>,
    override_file: Option<&AbsPath>,
    settings_args: &[toml::Table],
) -> buck2_error::Result<ResolvedSettings> {
    let (mut layers, mut reports) = parse_layers(repo_root, home_dir, rollout_layer(home_dir))?;
    // Outranks every on-disk layer so a harness can pin settings; only
    // per-command `--setting` flags are more specific.
    if let Some(path) = override_file {
        // Unlike ordinary layers, missing is an error: the caller asked for
        // this file by name, so running without it is the silent wrong answer.
        let table = parse_table(path)?
            .ok_or_else(|| SettingsError::OverrideFileMissing(path.display().to_string()))?;
        reports.push(SettingsLayerReport {
            origin: SettingOrigin::Local(path.to_owned()),
            applied: true,
            ignored: Vec::new(),
        });
        layers.push(SettingsLayer::new(
            SettingOrigin::Local(path.to_owned()),
            table,
        ));
    }
    for table in settings_args {
        reports.push(SettingsLayerReport {
            origin: SettingOrigin::CommandLine,
            applied: true,
            ignored: Vec::new(),
        });
        layers.push(SettingsLayer::setting_flag(table.clone()));
    }
    resolve_layers(layers, reports)
}

/// Parses TOML that is known to be valid. For tests.
pub fn table(content: &str) -> toml::Table {
    toml::from_str(content).unwrap()
}

#[cfg(test)]
mod tests {
    use buck2_core::fs::project::ProjectRootTemp;

    use super::*;
    use crate::args::SettingOverride;
    use crate::args::parse_setting_flag_arg;
    use crate::buckconfig::NoBuckconfig;
    use crate::metadata::SectionScope;
    use crate::metadata::SettingKeyMetadata;
    use crate::settings::testing::TestBuckSettingsData;
    use crate::settings::testing::TestSection;

    impl MergedSettings {
        fn provenance(&self, key: SettingKeyRef<'_>) -> Option<&SettingOrigin> {
            let value = match self.0.get(key.section)? {
                SourcedValue::Section(section) => section.0.get(key.name)?,
                SourcedValue::Leaf { .. } => return None,
            };
            match value {
                SourcedValue::Leaf { provenance, .. } => Some(provenance),
                SourcedValue::Section(_) => None,
            }
        }
    }

    const TEST_FLAG_METADATA: SettingKeyMetadata =
        SettingKeyMetadata::for_test("test_flag", &[OverrideSource::CommandLine]);

    const TEST_FLAG_NOT_OVERRIDABLE: &[SettingKeyMetadata] =
        &[SettingKeyMetadata::for_test("test_flag", &[])];

    const TEST_SETTINGS_METADATA: &[SettingKeyMetadata] = &[
        SettingKeyMetadata::for_test(
            "test_flag",
            &[OverrideSource::CommandLine, OverrideSource::LocalSettings],
        ),
        SettingKeyMetadata::for_test(
            "test_value",
            &[OverrideSource::CommandLine, OverrideSource::LocalSettings],
        ),
    ];

    fn resolve_with_metadata<T: DeserializeOwned>(
        layers: Vec<SettingsLayer>,
        metadata: &'static [SettingKeyMetadata],
    ) -> buck2_error::Result<T> {
        let merged = merge_layers(layers);
        merged.validate(&[SectionMetadata {
            section_name: "test_section",
            section_version: 0,
            scope: SectionScope::Daemon,
            doc: "",
            settings: metadata,
        }])?;
        merged.deserialize()
    }

    fn resolve_from_files_and_args(
        repo_files: &[(&str, &str)],
        home_files: &[(&str, &str)],
        settings_args: &[&str],
    ) -> buck2_error::Result<TestBuckSettingsData> {
        let repo = ProjectRootTemp::new()?;
        for (name, content) in repo_files {
            repo.write_file(name, content);
        }
        let home = ProjectRootTemp::new()?;
        for (name, content) in home_files {
            home.write_file(name, content);
        }
        let (mut layers, _reports) = parse_layers(
            repo.path().root().as_abs_path(),
            Some(home.path().root().as_abs_path()),
            RolloutLayer::default(),
        )?;
        layers.extend(
            settings_args
                .iter()
                .map(|arg| parse_setting_flag_arg(arg).map(SettingOverride::into_table))
                .map(|result| result.map(SettingsLayer::setting_flag))
                .collect::<buck2_error::Result<Vec<_>>>()?,
        );
        resolve_with_metadata(layers, TEST_SETTINGS_METADATA)
    }

    #[cfg(fbcode_build)]
    mod rollouts {
        use buck2_core::fs::project::ProjectRootTemp;
        use buck2_fs::paths::file_name::FileName;

        use super::*;
        use crate::path::DOT_BUCKSETTINGS_LOCAL;
        use crate::settings::ALL_SECTION_METADATA;

        const REPO_URL: &str = "https://repo/";

        fn compiled_version(section_name: &str) -> u32 {
            ALL_SECTION_METADATA
                .iter()
                .find(|section| section.section_name == section_name)
                .expect("The production registry contains the requested section")
                .section_version
        }

        fn rollout_toml(settings: &str) -> String {
            format!(
                "[log_download.{}]\n{settings}\n",
                compiled_version("log_download")
            )
        }

        fn hydration_rollout_toml(settings: &str) -> String {
            format!(
                "[hydration.{}]\n{settings}\n",
                compiled_version("hydration")
            )
        }

        fn rollout_path(home: &ProjectRootTemp) -> AbsPathBuf {
            home.path()
                .root()
                .as_abs_path()
                .join(buck2_wrapper_common::SETTINGS_ROLLOUTS_FILENAME)
        }

        fn write_rollout(home: &ProjectRootTemp, content: &[u8]) {
            std::fs::write(rollout_path(home).as_path(), content)
                .expect("Writing rollout test data should succeed");
        }

        fn settings_with_rollout(content: Option<&[u8]>) -> buck2_error::Result<BuckSettings> {
            let repo = ProjectRootTemp::new()?;
            repo.write_file(
                DOT_BUCKSETTINGS,
                &format!("[log_download]\nlog_url = \"{REPO_URL}\"\n"),
            );
            let home = ProjectRootTemp::new()?;
            if let Some(content) = content {
                write_rollout(&home, content);
            }
            parse_settings_with_home(
                repo.path().root().as_abs_path(),
                Some(home.path().root().as_abs_path()),
                None,
                &[],
            )
        }

        #[test]
        fn rollout_precedence() -> buck2_error::Result<()> {
            let repo = ProjectRootTemp::new()?;
            repo.write_file(
                DOT_BUCKSETTINGS,
                "[log_download]\nlog_url = \"https://repo/\"\n",
            );
            let home = ProjectRootTemp::new()?;
            write_rollout(
                &home,
                rollout_toml("log_url = \"https://rollout/\"").as_bytes(),
            );
            let repo_root = repo.path().root().as_abs_path();
            let home_dir = home.path().root().as_abs_path();

            let settings = parse_settings_with_home(repo_root, Some(home_dir), None, &[])?;
            assert_eq!(
                settings
                    .daemon
                    .log_download
                    .log_url(&mut NoBuckconfig)?
                    .as_deref(),
                Some("https://rollout/")
            );

            home.write_file(
                DOT_BUCKSETTINGS_LOCAL,
                "[log_download]\nlog_url = \"https://home-local/\"\n",
            );
            let settings = parse_settings_with_home(repo_root, Some(home_dir), None, &[])?;
            assert_eq!(
                settings
                    .daemon
                    .log_download
                    .log_url(&mut NoBuckconfig)?
                    .as_deref(),
                Some("https://home-local/")
            );

            repo.write_file(
                DOT_BUCKSETTINGS_LOCAL,
                "[log_download]\nlog_url = \"https://repo-local/\"\n",
            );
            let settings = parse_settings_with_home(repo_root, Some(home_dir), None, &[])?;
            assert_eq!(
                settings
                    .daemon
                    .log_download
                    .log_url(&mut NoBuckconfig)?
                    .as_deref(),
                Some("https://repo-local/")
            );

            let override_dir = ProjectRootTemp::new()?;
            override_dir.write_file(
                "override.toml",
                "[log_download]\nlog_url = \"https://override/\"\n",
            );
            let override_file = override_dir
                .path()
                .root()
                .as_abs_path()
                .join("override.toml");
            let settings =
                parse_settings_with_home(repo_root, Some(home_dir), Some(&override_file), &[])?;
            assert_eq!(
                settings
                    .daemon
                    .log_download
                    .log_url(&mut NoBuckconfig)?
                    .as_deref(),
                Some("https://override/")
            );

            let settings_args = [table("[log_download]\nlog_url = \"https://command-line/\"")];
            let settings = parse_settings_with_home(
                repo_root,
                Some(home_dir),
                Some(&override_file),
                &settings_args,
            )?;
            assert_eq!(
                settings
                    .daemon
                    .log_download
                    .log_url(&mut NoBuckconfig)?
                    .as_deref(),
                Some("https://command-line/")
            );

            // A missing override file is an error, not an absent layer.
            let missing = override_dir.path().root().as_abs_path().join("nope.toml");
            let result = parse_settings_with_home(repo_root, Some(home_dir), Some(&missing), &[]);
            assert!(
                result.is_err(),
                "a missing BUCK2_SETTINGS_OVERRIDE file must refuse, not fall through"
            );
            Ok(())
        }

        #[test]
        fn invalid_rollouts_fall_back_to_repo() -> buck2_error::Result<()> {
            let version = compiled_version("log_download");
            let cases = [
                ("invalid UTF-8", vec![0xff]),
                ("invalid TOML", b"= broken".to_vec()),
                (
                    "malformed selected version",
                    format!("[log_download]\n{version} = false").into_bytes(),
                ),
            ];

            for (label, content) in cases {
                let settings = settings_with_rollout(Some(&content))?;
                assert_eq!(
                    settings
                        .daemon
                        .log_download
                        .log_url(&mut NoBuckconfig)?
                        .as_deref(),
                    Some(REPO_URL),
                    "{label} must not affect other settings"
                );
            }
            Ok(())
        }

        #[test]
        fn unusable_rollout_keys_are_skipped_individually() -> buck2_error::Result<()> {
            let repo = ProjectRootTemp::new()?;
            repo.write_file(
                DOT_BUCKSETTINGS,
                &format!("[log_download]\nlog_url = \"{REPO_URL}\"\n"),
            );
            let home = ProjectRootTemp::new()?;
            write_rollout(
                &home,
                rollout_toml("log_url = \"https://rollout/\"\nnot_a_setting = true").as_bytes(),
            );
            let resolved = resolve_settings_with_home(
                repo.path().root().as_abs_path(),
                Some(home.path().root().as_abs_path()),
                None,
                &[],
            )?;
            assert_eq!(
                resolved
                    .settings
                    .daemon
                    .log_download
                    .log_url(&mut NoBuckconfig)?
                    .as_deref(),
                Some("https://rollout/")
            );
            let rollout = resolved
                .layers
                .iter()
                .find(|layer| matches!(layer.origin, SettingOrigin::Rollout(_)))
                .expect("the rollout cache is reported");
            assert!(rollout.applied);
            assert_eq!(
                rollout.ignored,
                [IgnoredSetting::key(
                    "log_download",
                    "not_a_setting",
                    "unknown setting"
                )]
            );
            Ok(())
        }

        #[test]
        fn read_failure_falls_back_to_repo() -> buck2_error::Result<()> {
            let repo = ProjectRootTemp::new()?;
            repo.write_file(
                DOT_BUCKSETTINGS,
                &format!("[log_download]\nlog_url = \"{REPO_URL}\"\n"),
            );
            let home = ProjectRootTemp::new()?;
            std::fs::create_dir(rollout_path(&home).as_path())
                .expect("Creating rollout test directory should succeed");

            let settings = parse_settings_with_home(
                repo.path().root().as_abs_path(),
                Some(home.path().root().as_abs_path()),
                None,
                &[],
            )?;
            assert_eq!(
                settings
                    .daemon
                    .log_download
                    .log_url(&mut NoBuckconfig)?
                    .as_deref(),
                Some(REPO_URL)
            );
            Ok(())
        }

        fn settings_for(rollout: Option<&str>) -> buck2_error::Result<BuckSettings> {
            let repo = ProjectRootTemp::new()?;
            repo.write_file(
                DOT_BUCKSETTINGS,
                "[log_download]\nlog_url = \"https://repo/\"\n",
            );
            let home = ProjectRootTemp::new()?;
            if let Some(rollout) = rollout {
                write_rollout(&home, rollout.as_bytes());
            }
            home.write_file(
                DOT_BUCKSETTINGS_LOCAL,
                "[log_download]\nlog_use_manifold = true\n",
            );
            parse_settings_with_home(
                repo.path().root().as_abs_path(),
                Some(home.path().root().as_abs_path()),
                None,
                &[],
            )
        }

        #[test]
        fn rollout_value_changes_daemon_settings() -> buck2_error::Result<()> {
            let before = settings_for(Some(&rollout_toml("log_url = \"https://before/\"")))?;
            let after = settings_for(Some(&rollout_toml("log_url = \"https://after/\"")))?;
            assert!(!after.daemon.changes_from(&before.daemon).is_empty());
            Ok(())
        }

        #[test]
        fn hydration_rollout_scopes_idle_page_out() -> buck2_error::Result<()> {
            let settings = settings_for(Some(&hydration_rollout_toml(
                "enable_paging = true\npage_out_on_idle = true\npage_out_on_idle_isolation_dir_scope = \"non_default\"",
            )))?;
            let hydration = &settings.daemon.hydration;
            assert!(hydration.enable_paging());
            assert!(hydration.page_out_on_idle());
            assert!(!hydration.page_out_on_idle_applies_to_isolation_dir(
                FileName::new("v2").expect("The default isolation dir should be valid")
            ));
            assert!(hydration.page_out_on_idle_applies_to_isolation_dir(
                FileName::new("custom").expect("The test isolation dir should be valid")
            ));
            Ok(())
        }

        #[test]
        fn older_agent_interactive_stanza_applies() -> buck2_error::Result<()> {
            // A stanza for an older version keeps applying; the guards keep
            // needing their own switch on top of the master one.
            let settings = settings_with_rollout(Some(b"[agent_interactive.0]\nenabled = true"))?;
            let guards = &settings.invocation.agent_interactive;
            assert!(guards.enabled());
            assert!(!guards.killall_enabled());
            assert!(!guards.clean_enabled());
            Ok(())
        }

        #[test]
        fn agent_build_rollout_preserves_existing_guards() -> buck2_error::Result<()> {
            for enabled in [false, true] {
                let rollout = format!(
                    "[agent_interactive.1]\nenabled = true\nclean_enabled = true\n\
                     [agent_build.0]\nenabled = {enabled}"
                );
                let settings = settings_with_rollout(Some(rollout.as_bytes()))?;
                assert!(settings.invocation.agent_interactive.clean_enabled());
                assert_eq!(settings.invocation.agent_build.enabled(), enabled);
            }
            Ok(())
        }
    }

    #[cfg(not(fbcode_build))]
    #[test]
    fn test_oss_ignores_rollout_file() -> buck2_error::Result<()> {
        let repo = ProjectRootTemp::new()?;
        let home = ProjectRootTemp::new()?;
        home.write_file(
            buck2_wrapper_common::SETTINGS_ROLLOUTS_FILENAME,
            "[log_download.0]\nlog_url = \"https://rollout/\"",
        );
        let settings = parse_settings_with_home(
            repo.path().root().as_abs_path(),
            Some(home.path().root().as_abs_path()),
            None,
            &[],
        )?;
        assert_eq!(
            settings
                .daemon
                .log_download
                .log_url(&mut NoBuckconfig)?
                .as_deref(),
            None
        );
        Ok(())
    }

    #[test]
    fn test_parse_table_missing_file() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let path = fs.path().root().as_abs_path().join(".bucksettings.toml");
        assert_eq!(parse_table(&path)?, None);
        Ok(())
    }

    #[test]
    fn test_parse_table_invalid_format() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let path = fs.path().root().as_abs_path().join(".bucksettings.toml");
        fs.write_file(".bucksettings.toml", "= broken");
        assert!(parse_table(&path).is_err());
        Ok(())
    }

    #[test]
    fn test_parse_table() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let path = fs.path().root().as_abs_path().join(".bucksettings.toml");

        fs.write_file(".bucksettings.toml", "");
        assert_eq!(parse_table(&path)?, Some(toml::Table::new()));

        let sectioned = "[test_section]\ntest_flag = true\ntest_value = \"x\"";
        fs.write_file(".bucksettings.toml", sectioned);
        assert_eq!(parse_table(&path)?, Some(table(sectioned)));

        Ok(())
    }

    #[test]
    fn test_merge() {
        let merged = merge_layers(vec![
            SettingsLayer::setting_flag(table("a = 1\nb = 2\n[s]\nx = 1\ny = 2")),
            SettingsLayer::setting_flag(table("b = 3\nc = 4\n[s]\ny = 3")),
        ]);
        assert_eq!(
            merged.into_table(),
            table("a = 1\nb = 3\nc = 4\n[s]\nx = 1\ny = 3")
        );
    }

    #[test]
    fn test_layer_sources_and_winning_origins() -> buck2_error::Result<()> {
        let repo = ProjectRootTemp::new()?;
        repo.write_file(".bucksettings.toml", "[test_section]\ntest_flag = true");
        repo.write_file(
            ".bucksettings.local.toml",
            "[test_section]\ntest_flag = false",
        );
        let home = ProjectRootTemp::new()?;
        home.write_file(
            ".bucksettings.local.toml",
            "[test_section]\ntest_value = \"home\"",
        );
        let rollout_path = home
            .path()
            .root()
            .as_abs_path()
            .join(buck2_wrapper_common::SETTINGS_ROLLOUTS_FILENAME);

        let (mut layers, reports) = parse_layers(
            repo.path().root().as_abs_path(),
            Some(home.path().root().as_abs_path()),
            RolloutLayer {
                layer: Some(SettingsLayer::new(
                    SettingOrigin::Rollout(rollout_path.clone()),
                    table("[test_section]\ntest_value = \"rollout\""),
                )),
                ignored: Vec::new(),
            },
        )?;
        let repository =
            SettingOrigin::Repository(repo.path().root().as_abs_path().join(DOT_BUCKSETTINGS));
        assert_eq!(
            reports[0],
            SettingsLayerReport {
                origin: repository.clone(),
                applied: true,
                ignored: Vec::new(),
            }
        );
        // The rollout cache is only a layer in Meta-internal builds.
        if cfg!(fbcode_build) {
            assert_eq!(
                reports[1],
                SettingsLayerReport {
                    origin: SettingOrigin::Rollout(rollout_path.clone()),
                    applied: true,
                    ignored: Vec::new(),
                }
            );
        }
        assert!(
            reports.iter().any(|report| {
                matches!(report.origin, SettingOrigin::Local(_)) && report.applied
            })
        );
        assert_eq!(layers[0].provenance.as_ref(), &repository);
        assert_eq!(layers[0].provenance.setting_source(), SettingSource::Base);
        assert_eq!(
            layers[1].provenance.as_ref(),
            &SettingOrigin::Rollout(rollout_path)
        );
        assert_eq!(
            layers[1].provenance.setting_source(),
            SettingSource::Rollout
        );
        assert_eq!(
            layers[2].provenance.as_ref(),
            &SettingOrigin::Local(
                home.path()
                    .root()
                    .as_abs_path()
                    .join(".bucksettings.local.toml")
            )
        );
        assert_eq!(
            layers[3].provenance.as_ref(),
            &SettingOrigin::Local(
                repo.path()
                    .root()
                    .as_abs_path()
                    .join(".bucksettings.local.toml")
            )
        );
        layers.push(SettingsLayer::setting_flag(table(
            "[test_section]\ntest_flag = true",
        )));

        let merged = merge_layers(layers);
        assert_eq!(
            merged.provenance(SettingKeyRef {
                section: "test_section",
                name: "test_flag",
            }),
            Some(&SettingOrigin::CommandLine)
        );
        assert_eq!(
            merged
                .provenance(SettingKeyRef {
                    section: "test_section",
                    name: "test_flag",
                })
                .map(SettingOrigin::setting_source),
            Some(SettingSource::Override(OverrideSource::CommandLine))
        );
        assert_eq!(
            merged
                .provenance(SettingKeyRef {
                    section: "test_section",
                    name: "test_value",
                })
                .map(SettingOrigin::setting_source),
            Some(SettingSource::Override(OverrideSource::LocalSettings))
        );
        assert_eq!(
            merged.provenance(SettingKeyRef {
                section: "test_section",
                name: "test_value",
            }),
            Some(&SettingOrigin::Local(
                home.path()
                    .root()
                    .as_abs_path()
                    .join(".bucksettings.local.toml")
            ))
        );
        Ok(())
    }

    #[test]
    fn test_rejects_disallowed_local_settings_override() {
        let local = ProjectRootTemp::new().unwrap();
        let path = local
            .path()
            .root()
            .as_abs_path()
            .join(".bucksettings.local.toml");
        let error = resolve_with_metadata::<TestBuckSettingsData>(
            vec![SettingsLayer::new(
                SettingOrigin::Local(path.clone()),
                table("[test_section]\ntest_flag = true"),
            )],
            &[TEST_FLAG_METADATA],
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "Buck setting `test_section.test_flag` cannot be overridden from local settings file `{}`",
                path.display()
            )
        );
    }

    #[test]
    fn test_rejects_disallowed_command_line_override() {
        let error = resolve_with_metadata::<TestBuckSettingsData>(
            vec![SettingsLayer::setting_flag(table(
                "[test_section]\ntest_flag = true",
            ))],
            TEST_FLAG_NOT_OVERRIDABLE,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Buck setting `test_section.test_flag` cannot be overridden from `--setting`"
        );
    }

    #[test]
    fn test_rollout_bypasses_override_policy() -> buck2_error::Result<()> {
        let home = ProjectRootTemp::new()?;
        let resolved = resolve_with_metadata::<TestBuckSettingsData>(
            vec![SettingsLayer::new(
                SettingOrigin::Rollout(
                    home.path()
                        .root()
                        .as_abs_path()
                        .join(buck2_wrapper_common::SETTINGS_ROLLOUTS_FILENAME),
                ),
                table("[test_section]\ntest_flag = true"),
            )],
            TEST_FLAG_NOT_OVERRIDABLE,
        )?;
        assert_eq!(resolved.test_section.unwrap().test_flag, Some(true));
        Ok(())
    }

    #[test]
    fn test_top_level_keys_are_rejected() {
        let error = resolve_setting_flags(vec![table("log_use_manifold = false")]).unwrap_err();
        assert!(error.to_string().contains("log_use_manifold"), "{error}");
    }

    #[test]
    fn test_repo_root_settings_always_valid() -> buck2_error::Result<()> {
        let repo = ProjectRootTemp::new()?;
        let base = resolve_with_metadata::<TestBuckSettingsData>(
            vec![SettingsLayer::new(
                SettingOrigin::Repository(
                    repo.path().root().as_abs_path().join(".bucksettings.toml"),
                ),
                table("[test_section]\ntest_flag = true"),
            )],
            &[TEST_FLAG_METADATA],
        )?;
        assert_eq!(base.test_section.unwrap().test_flag, Some(true));

        Ok(())
    }

    #[test]
    fn test_overridden_disallowed_source_is_ignored() -> buck2_error::Result<()> {
        let local = ProjectRootTemp::new()?;
        let shadowed = resolve_with_metadata::<TestBuckSettingsData>(
            vec![
                SettingsLayer::new(
                    SettingOrigin::Local(
                        local
                            .path()
                            .root()
                            .as_abs_path()
                            .join(".bucksettings.local.toml"),
                    ),
                    table("[test_section]\ntest_flag = false"),
                ),
                SettingsLayer::setting_flag(table("[test_section]\ntest_flag = true")),
            ],
            &[TEST_FLAG_METADATA],
        )?;
        assert_eq!(shadowed.test_section.unwrap().test_flag, Some(true));
        Ok(())
    }

    #[test]
    fn test_overridden_invalid_type_is_ignored() -> buck2_error::Result<()> {
        let resolved = resolve_with_metadata::<TestBuckSettingsData>(
            vec![
                SettingsLayer::setting_flag(table("[test_section]\ntest_flag = \"invalid\"")),
                SettingsLayer::setting_flag(table("[test_section]\ntest_flag = true")),
            ],
            &[TEST_FLAG_METADATA],
        )?;
        assert_eq!(resolved.test_section.unwrap().test_flag, Some(true));
        Ok(())
    }

    #[test]
    fn test_unknown_key_uses_deserialization_error() {
        let error = resolve_setting_flags(vec![table("nonexistent = true")]).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn test_home_local_override_repo() -> buck2_error::Result<()> {
        let resolved = resolve_from_files_and_args(
            &[(
                ".bucksettings.toml",
                "[test_section]\ntest_flag = true\ntest_value = \"repo\"",
            )],
            &[(
                ".bucksettings.local.toml",
                "[test_section]\ntest_flag = false",
            )],
            &[],
        )?;
        assert_eq!(
            resolved,
            TestBuckSettingsData {
                test_section: Some(TestSection {
                    test_flag: Some(false),
                    test_value: Some("repo".to_owned()),
                }),
            }
        );
        Ok(())
    }

    #[test]
    fn test_repo_local_overrides_home_local_and_repo_root() -> buck2_error::Result<()> {
        let resolved = resolve_from_files_and_args(
            &[
                (".bucksettings.toml", "[test_section]\ntest_flag = true"),
                (
                    ".bucksettings.local.toml",
                    "[test_section]\ntest_flag = false\ntest_value = \"repo_local\"",
                ),
            ],
            &[(
                ".bucksettings.local.toml",
                "[test_section]\ntest_value = \"home_local\"",
            )],
            &[],
        )?;
        assert_eq!(
            resolved,
            TestBuckSettingsData {
                test_section: Some(TestSection {
                    test_flag: Some(false),
                    test_value: Some("repo_local".to_owned()),
                }),
            }
        );
        Ok(())
    }

    #[test]
    fn test_settings_args_override_all() -> buck2_error::Result<()> {
        let resolved = resolve_from_files_and_args(
            &[
                (
                    ".bucksettings.toml",
                    "[test_section]\ntest_value = \"repo\"",
                ),
                (
                    ".bucksettings.local.toml",
                    "[test_section]\ntest_value = \"repo_local\"",
                ),
            ],
            &[(
                ".bucksettings.local.toml",
                "[test_section]\ntest_value = \"home_local\"",
            )],
            &["test_section.test_value=command_line"],
        )?;
        assert_eq!(
            resolved.test_section,
            Some(TestSection {
                test_flag: None,
                test_value: Some("command_line".to_owned()),
            })
        );
        Ok(())
    }

    #[test]
    fn test_settings_args_ordering() -> buck2_error::Result<()> {
        let resolved = resolve_from_files_and_args(
            &[],
            &[],
            &[
                "test_section.test_flag=false",
                "test_section.test_flag=true",
            ],
        )?;
        assert_eq!(
            resolved.test_section,
            Some(TestSection {
                test_flag: Some(true),
                test_value: None,
            })
        );
        Ok(())
    }
}
