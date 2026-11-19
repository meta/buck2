/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::Context as _;
use globset::Candidate;
use globset::Glob;
use globset::GlobSet;
use globset::GlobSetBuilder;
use serde::Deserialize;
use serde::Deserializer;

use crate::sort_key::ListSortKeys;

fn deserialize_keys_as_set<'de, D>(deserializer: D) -> Result<HashSet<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let map: HashMap<String, bool> = HashMap::deserialize(deserializer)?;
    Ok(map.into_keys().collect())
}

/// Deserialize a JSON array of glob patterns into a compiled [`GlobSet`].
///
/// Compiling at load time means an invalid pattern fails config parsing
/// loudly, rather than silently never matching at format time.
fn compile_globset<'de, D>(deserializer: D) -> Result<GlobSet, D::Error>
where
    D: Deserializer<'de>,
{
    let patterns = Vec::<String>::deserialize(deserializer)?;
    let mut builder = GlobSetBuilder::new();
    for pattern in &patterns {
        builder.add(Glob::new(pattern).map_err(serde::de::Error::custom)?);
    }
    builder.build().map_err(serde::de::Error::custom)
}

/// A per-file override of formatting behavior, selected by glob.
///
/// Overrides are matched against the file path in document order; for a given
/// knob, the last matching override that sets it wins (see
/// [`Config::sort_flags`]). A knob left unset falls through to an
/// earlier match, and ultimately to the built-in default.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Override {
    #[serde(deserialize_with = "compile_globset")]
    files: GlobSet,
    sort_list_args: Option<bool>,
    sort_kwargs: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Config {
    #[serde(
        rename = "IsSortableListArg",
        deserialize_with = "deserialize_keys_as_set"
    )]
    sortable_args: HashSet<String>,
    #[serde(
        rename = "SortableBlacklist",
        deserialize_with = "deserialize_keys_as_set"
    )]
    sortable_blocklist: HashSet<String>,
    name_priority: HashMap<String, i32>,
    #[serde(default)]
    list_sort_keys: ListSortKeys,
    #[serde(default)]
    overrides: Vec<Override>,
}

/// Which sorting passes apply to a given file, resolved from the matching
/// config overrides.
pub(crate) struct SortFlags {
    /// Sort allowlisted rule arguments (`deps`, `srcs`, ...). Explicit
    /// `# keep sorted` lists are sorted regardless of this flag.
    pub(crate) list_args: bool,
    /// Sort keyword arguments by priority.
    pub(crate) kwargs: bool,
}

impl Config {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file `{}`", path.display()))?;
        let config: Config = serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse config file `{}`", path.display()))?;
        Ok(config)
    }

    /// Names of list-valued rule arguments whose elements should be sorted.
    pub(crate) fn sortable_args(&self) -> &HashSet<String> {
        &self.sortable_args
    }

    /// `macro.arg` combinations excluded from list-argument sorting.
    pub(crate) fn sortable_blocklist(&self) -> &HashSet<String> {
        &self.sortable_blocklist
    }

    /// Keyword-argument sort priorities; lower values sort first.
    pub(crate) fn name_priority(&self) -> &HashMap<String, i32> {
        &self.name_priority
    }

    pub(crate) fn list_sort_keys(&self) -> Option<&ListSortKeys> {
        (!self.list_sort_keys.is_empty()).then_some(&self.list_sort_keys)
    }

    /// Resolve which sorting passes apply to `path`. For each knob, the last
    /// matching override that sets it wins, defaulting to `true`. The path is
    /// matched once via a shared [`Candidate`] reused across all overrides.
    /// Note: explicit `# keep sorted` lists are sorted regardless of
    /// [`SortFlags::list_args`].
    pub(crate) fn sort_flags(&self, path: &Path) -> SortFlags {
        let candidate = Candidate::new(path);
        let mut flags = SortFlags {
            list_args: true,
            kwargs: true,
        };
        for o in &self.overrides {
            if o.files.is_match_candidate(&candidate) {
                if let Some(v) = o.sort_list_args {
                    flags.list_args = v;
                }
                if let Some(v) = o.sort_kwargs {
                    flags.kwargs = v;
                }
            }
        }
        flags
    }
}

#[cfg(test)]
impl Config {
    /// Build a config directly from its sorting tables with no overrides.
    /// Used by autofix unit tests that exercise sorting logic in isolation.
    pub(crate) fn from_tables(
        sortable_args: HashSet<String>,
        sortable_blocklist: HashSet<String>,
        name_priority: HashMap<String, i32>,
    ) -> Self {
        Self {
            sortable_args,
            sortable_blocklist,
            name_priority,
            list_sort_keys: ListSortKeys::default(),
            overrides: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ignores_unknown_fields() {
        let json = r#"{
            "IsLabelArg": {"some_label": true},
            "LabelBlacklist": {"blocked": true},
            "IsSortableListArg": {"deps": true, "srcs": true},
            "SortableBlacklist": {"genrule.srcs": true},
            "SortableWhitelist": {"allowed": true},
            "NamePriority": {"name": -165, "deps": 4}
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(
            config.sortable_args(),
            &HashSet::from(["deps".to_owned(), "srcs".to_owned()])
        );
        assert_eq!(
            config.sortable_blocklist(),
            &HashSet::from(["genrule.srcs".to_owned()])
        );
        assert_eq!(
            config.name_priority(),
            &HashMap::from([("name".to_owned(), -165), ("deps".to_owned(), 4)])
        );
        assert!(config.overrides.is_empty());
    }

    #[test]
    fn test_only_required_fields() {
        let json = r#"{
            "IsSortableListArg": {},
            "SortableBlacklist": {},
            "NamePriority": {}
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert!(config.sortable_args().is_empty());
        assert!(config.sortable_blocklist().is_empty());
        assert!(config.name_priority().is_empty());
        assert!(config.list_sort_keys().is_none());
        assert!(config.overrides.is_empty());
    }

    #[test]
    fn test_list_sort_keys_are_optional_and_validated() {
        let config: Config = serde_json::from_str(
            r#"{
                "IsSortableListArg": {},
                "SortableBlacklist": {},
                "NamePriority": {},
                "ListSortKeys": {
                    "items": "call_name",
                    "special.items": {"first_of": ["string", {"tuple_item": 0}]}
                }
            }"#,
        )
        .expect("valid sort keys should parse");

        let sort_keys = config.list_sort_keys().expect("configured sort keys");
        assert!(sort_keys.for_arg("items").is_some());
    }

    #[test]
    fn test_invalid_list_sort_key_fails_config_parsing() {
        let result: Result<Config, _> = serde_json::from_str(
            r#"{
                "IsSortableListArg": {},
                "SortableBlacklist": {},
                "NamePriority": {},
                "ListSortKeys": {"items": {"first_of": []}}
            }"#,
        );

        result.expect_err("empty `first_of` should be rejected");
    }

    #[test]
    fn test_missing_required_field_fails() {
        let json = r#"{
            "IsSortableListArg": {},
            "SortableBlacklist": {}
        }"#;

        let result: Result<Config, _> = serde_json::from_str(json);
        let err = result.unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("NamePriority"),
            "Error message should mention missing field 'NamePriority', got: {}",
            message
        );
    }

    #[test]
    fn test_invalid_value_type_fails() {
        let json = r#"{
            "IsSortableListArg": {"deps": "not_a_bool"},
            "SortableBlacklist": {},
            "NamePriority": {}
        }"#;

        let result: Result<Config, _> = serde_json::from_str(json);
        let err = result.unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("bool"),
            "Error message should mention expected type 'bool', got: {}",
            message
        );
    }

    #[test]
    fn test_no_overrides_sorting_enabled_everywhere() {
        let json = r#"{
            "IsSortableListArg": {},
            "SortableBlacklist": {},
            "NamePriority": {}
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();

        let bzl = config.sort_flags(Path::new("foo.bzl"));
        assert!(bzl.list_args && bzl.kwargs);
        let build = config.sort_flags(Path::new("BUCK"));
        assert!(build.list_args && build.kwargs);
    }

    #[test]
    fn test_bzl_override_disables_rule_arg_sorting() {
        let json = r#"{
            "IsSortableListArg": {},
            "SortableBlacklist": {},
            "NamePriority": {},
            "Overrides": [
                {"Files": ["*.bzl"], "SortListArgs": false, "SortKwargs": false}
            ]
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();

        // .bzl files: both sorts disabled by the override.
        let bzl = config.sort_flags(Path::new("foo.bzl"));
        assert!(!bzl.list_args);
        assert!(!bzl.kwargs);
        // Nested .bzl paths match too (the `*` glob spans `/`).
        let nested = config.sort_flags(Path::new("a/b/c.bzl"));
        assert!(!nested.list_args);
        assert!(!nested.kwargs);
        // BUILD-like files are unaffected and keep sorting enabled.
        let build = config.sort_flags(Path::new("BUCK"));
        assert!(build.list_args);
        assert!(config.sort_flags(Path::new("TARGETS")).kwargs);
    }

    #[test]
    fn test_override_unset_knob_uses_default() {
        // The override sets only SortKwargs; SortListArgs stays at its default.
        let json = r#"{
            "IsSortableListArg": {},
            "SortableBlacklist": {},
            "NamePriority": {},
            "Overrides": [
                {"Files": ["*.bzl"], "SortKwargs": false}
            ]
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();

        let flags = config.sort_flags(Path::new("foo.bzl"));
        assert!(
            flags.list_args,
            "an unset knob should fall back to the default (true)"
        );
        assert!(!flags.kwargs);
    }

    #[test]
    fn test_last_matching_override_wins() {
        let json = r#"{
            "IsSortableListArg": {},
            "SortableBlacklist": {},
            "NamePriority": {},
            "Overrides": [
                {"Files": ["*.bzl"], "SortKwargs": false},
                {"Files": ["special.bzl"], "SortKwargs": true}
            ]
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();

        assert!(
            !config.sort_flags(Path::new("other.bzl")).kwargs,
            "only the first override matches other.bzl"
        );
        assert!(
            config.sort_flags(Path::new("special.bzl")).kwargs,
            "the later override wins when both match special.bzl"
        );
    }

    #[test]
    fn test_invalid_glob_fails() {
        let json = r#"{
            "IsSortableListArg": {},
            "SortableBlacklist": {},
            "NamePriority": {},
            "Overrides": [
                {"Files": ["a[b"], "SortKwargs": false}
            ]
        }"#;

        let result: Result<Config, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "an unclosed character class should fail config parsing"
        );
    }

    #[test]
    fn test_glob_with_json_escape_parses() {
        // A pattern written with a JSON escape (an escaped `/`) cannot be
        // borrowed zero-copy from the input, so the override value must
        // deserialize owned. Regression test: it must parse rather than error
        // on a transient (unescaped) string.
        let json = r#"{
            "IsSortableListArg": {},
            "SortableBlacklist": {},
            "NamePriority": {},
            "Overrides": [
                {"Files": ["a\\/b.bzl"], "SortKwargs": false}
            ]
        }"#;

        let result: Result<Config, _> = serde_json::from_str(json);
        assert!(
            result.is_ok(),
            "a glob containing a JSON escape must parse, got: {result:?}"
        );
    }
}
