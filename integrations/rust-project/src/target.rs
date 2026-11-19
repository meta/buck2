/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;

use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::Error as _;
use serde::de::MapAccess;
use serde::de::SeqAccess;
use serde::de::Visitor;

use crate::path::canonicalize_to_vcs_path;
use crate::project_json::Edition;

#[derive(Serialize, Debug, Default, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub(crate) struct Target(String);

impl Target {
    pub(crate) fn new<T>(target: T) -> Target
    where
        T: Into<String>,
    {
        let target: String = target.into();
        let target = target.split(' ').next().expect("Unable to get raw label");
        Target(target.to_owned())
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let string: String = Deserialize::deserialize(deserializer)?;
        Ok(Target::new(string))
    }
}

impl Deref for Target {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.0.as_str()
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Allows Target to be passed as an argument to Command.
impl AsRef<OsStr> for Target {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(&self.0)
    }
}

impl AsRef<Path> for Target {
    fn as_ref(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl AsRef<[u8]> for Target {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct MacroOutput {
    pub(crate) actual: Target,
    pub(crate) dylib: PathBuf,
}
/// The kind of a Rust target, as reported by the `resolve_deps.bxl` prelude script.
///
/// This binary is shipped as a pinned prebuilt (via DotSlash) that is versioned
/// independently of the prelude BXL it invokes, so a given binary may run against
/// an older or newer prelude. Accept both the semantic kind names (`bin`/`lib`/`test`)
/// emitted by newer preludes and the legacy fully-qualified rule types emitted by
/// older ones.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) enum Kind {
    #[serde(rename = "bin", alias = "prelude//rules.bzl:rust_binary")]
    Binary,
    #[serde(rename = "lib", alias = "prelude//rules.bzl:rust_library")]
    Library,
    #[serde(rename = "test", alias = "prelude//rules.bzl:rust_test")]
    Test,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct TargetInfo {
    pub(crate) name: String,
    /// The target identifier, e.g. `fbcode//buck2/integrations/rust-project:rust-project`.
    ///
    /// See also <https://buck2.build/docs/concepts/labels/>
    pub(crate) label: String,
    /// A list of tags, e.g. ["xplat", "split-dwarf"]
    ///
    /// This is generic metadata about the target.
    pub(crate) labels: Vec<String>,

    pub(crate) kind: Kind,
    pub(crate) edition: Option<Edition>,
    pub(crate) srcs: Vec<PathBuf>,
    /// Mapped srcs are effectively aliases. The key is a buck target
    /// of some kind, and the value is a path/filename that can be
    /// referred to in the rest of the rule.
    ///
    /// Asking buck to build the targets and tell us the output path
    /// is how we are able to support generated sources.
    pub(crate) mapped_srcs: FxHashMap<PathBuf, PathBuf>,
    #[serde(rename = "crate")]
    pub(crate) crate_name: Option<String>,
    pub(crate) crate_dynamic: Option<PathBuf>,
    pub(crate) crate_root: PathBuf,
    pub(crate) deps: Vec<Target>,
    #[serde(rename = "tests")]
    pub(crate) test_deps: Vec<Target>,
    // Optional set of renamed crates. in buck2, these are not unified with
    // `buck.direct_dependencies` and are instead a separate entry.
    #[serde(deserialize_with = "deserialize_named_deps")]
    pub(crate) named_deps: FxHashMap<String, Target>,
    pub(crate) proc_macro: Option<bool>,
    // Set of features enabled for this crate.
    pub(crate) features: Vec<String>,
    pub(crate) env: FxHashMap<String, String>,
    // The ensured folder containing symlinks to all sources
    pub(crate) source_folder: PathBuf,
    pub(crate) project_relative_buildfile: PathBuf,
    pub(crate) in_workspace: bool,
    pub(crate) rustc_flags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct AliasedTargetInfo {
    pub(crate) actual: Option<Target>,
}

impl TargetInfo {
    pub(crate) fn crate_name(&self) -> String {
        if let Some(crate_dynamic) = &self.crate_dynamic {
            if let Ok(contents) = fs::read_to_string(crate_dynamic) {
                return contents.trim().to_owned();
            }
        }
        self.crate_name.as_deref().map_or_else(
            || self.name.as_str().replace('-', "_"),
            |crate_name| crate_name.to_owned(),
        )
    }

    pub(crate) fn display_name(&self) -> String {
        let starts_with_version = self.name.starts_with(|c: char| c.is_ascii_digit())
            || self
                .name
                .strip_prefix('_')
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
        if starts_with_version {
            // For targets of the form foo:1.2.3, foo:_1.2.3, or
            // foo:0.3.0-pre.0, the buck name isn't useful as a display name.
            self.crate_name()
        } else {
            self.name
                .strip_suffix("-unittest")
                .unwrap_or(&self.name)
                .to_owned()
        }
    }

    pub(crate) fn root_module(&self, project_root: &Path) -> PathBuf {
        let p = self.source_folder.join(&self.crate_root);
        canonicalize_to_vcs_path(&p, project_root)
    }

    pub(crate) fn cfg(&self) -> Vec<String> {
        // we need to take the existing features and prefix `feature=`
        let feature_cfgs = self.features.iter().map(|f| format!("feature=\"{f}\""));

        // parse out rustc --cfg= flags
        let rustc_flags_cfgs = self.rustc_flags.iter().flat_map(|flag| {
            if let Some(atfile) = flag.strip_prefix("@") {
                // If we fail to expand, continue anyways to provide a nearly working experience.
                expand_atfile(Path::new(atfile)).unwrap_or_default()
            } else if let Some(cfg) = flag.strip_prefix("--cfg=") {
                vec![cfg.to_owned()]
            } else {
                vec![]
            }
        });

        feature_cfgs
            .chain(rustc_flags_cfgs)
            .collect::<Vec<String>>()
    }

    /// Is this a reindeer-vendored third-party crate?
    ///
    /// Reindeer buckifies vendored crates under a fixed set of package roots,
    /// authoritatively defined by `_assert_is_allowed_third_party_root` in
    /// `fbsource//tools/build_defs/third_party:rust_third_party.bzl` (the
    /// `rust_third_party` macros fail the build outside these roots).
    pub(crate) fn is_reindeer_third_party(&self) -> bool {
        // Strip the cell (e.g. `fbsource//`) and the target name (`:foo`) to get
        // the buck package path.
        let package = self
            .label
            .split_once("//")
            .map_or(self.label.as_str(), |(_cell, rest)| rest);
        let package = package.split(':').next().unwrap_or(package);

        package == "third-party/rust"
            || package == "third-party/rust/top"
            || package.starts_with("third-party/rust/vendor/")
            || package == "third-party/rust-toolchain"
            || package.starts_with("third-party/rust-toolchain/")
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct ExpandedAndResolved {
    pub(crate) expanded_targets: Vec<Target>,
    pub(crate) queried_proc_macros: FxHashMap<Target, MacroOutput>,
    pub(crate) resolved_deps: FxHashMap<Target, TargetInfo>,
}

fn deserialize_named_deps<'de, D>(deserializer: D) -> Result<FxHashMap<String, Target>, D::Error>
where
    D: Deserializer<'de>,
{
    struct NamedDepsVisitor;

    impl<'de> Visitor<'de> for NamedDepsVisitor {
        type Value = FxHashMap<String, Target>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("dict or list")
        }

        fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            FxHashMap::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let names: PathBuf = seq
                .next_element()?
                .ok_or_else(|| S::Error::invalid_length(0, &self))?;
            let content = fs::read_to_string(&names).map_err(|e| {
                S::Error::custom(format!("failed to read {}: {}", names.display(), e))
            })?;
            let mut lines = content.lines();

            let mut named_deps = FxHashMap::default();
            while let Some(target) = seq.next_element()? {
                let name = lines.next().ok_or_else(|| {
                    S::Error::custom(format!("not enough lines in {}", names.display()))
                })?;
                named_deps.insert(name.to_owned(), target);
            }
            Ok(named_deps)
        }
    }

    deserializer.deserialize_any(NamedDepsVisitor)
}

fn expand_atfile(path: &Path) -> Result<Vec<String>, anyhow::Error> {
    let contents = fs::read_to_string(path)?;
    let flags = contents
        .lines()
        .filter_map(|flag| flag.strip_prefix("--cfg=").map(str::to_owned));
    Ok(flags.collect::<Vec<String>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kind_deserializes_semantic_values() -> Result<(), serde_json::Error> {
        for (value, expected) in [
            (r#""bin""#, Kind::Binary),
            (r#""lib""#, Kind::Library),
            (r#""test""#, Kind::Test),
            // Legacy fully-qualified rule types emitted by older preludes.
            (r#""prelude//rules.bzl:rust_binary""#, Kind::Binary),
            (r#""prelude//rules.bzl:rust_library""#, Kind::Library),
            (r#""prelude//rules.bzl:rust_test""#, Kind::Test),
        ] {
            assert_eq!(serde_json::from_str::<Kind>(value)?, expected);
        }

        assert!(serde_json::from_str::<Kind>(r#""not_a_kind""#).is_err());

        Ok(())
    }

    #[test]
    fn test_cfg() {
        let info = TargetInfo {
            name: "bar".to_owned(),
            label: "bar".to_owned(),
            labels: vec![],
            kind: Kind::Library,
            edition: None,
            srcs: vec![],
            mapped_srcs: FxHashMap::default(),
            crate_name: None,
            crate_dynamic: None,
            crate_root: PathBuf::default(),
            deps: vec![],
            test_deps: vec![],
            named_deps: FxHashMap::default(),
            proc_macro: None,
            features: vec!["foo_feature".to_owned()],
            env: FxHashMap::default(),
            source_folder: PathBuf::from("/tmp"),
            project_relative_buildfile: PathBuf::from("bar/BUCK"),
            in_workspace: false,
            rustc_flags: vec!["--cfg=foo_cfg".to_owned(), "--other".to_owned()],
        };

        assert_eq!(
            info.cfg(),
            vec!["feature=\"foo_feature\"".to_owned(), "foo_cfg".to_owned()]
        );
    }

    #[test]
    fn test_is_reindeer_third_party() {
        let with_label = |label: &str| TargetInfo {
            name: "foo".to_owned(),
            label: label.to_owned(),
            labels: vec![],
            kind: Kind::Library,
            edition: None,
            srcs: vec![],
            mapped_srcs: FxHashMap::default(),
            crate_name: None,
            crate_dynamic: None,
            crate_root: PathBuf::default(),
            deps: vec![],
            test_deps: vec![],
            named_deps: FxHashMap::default(),
            proc_macro: None,
            features: vec![],
            env: FxHashMap::default(),
            source_folder: PathBuf::from("/tmp"),
            project_relative_buildfile: PathBuf::from("foo/BUCK"),
            in_workspace: false,
            rustc_flags: vec![],
        };

        assert!(with_label("fbsource//third-party/rust/vendor/tokio:1").is_reindeer_third_party());
        assert!(with_label("fbsource//third-party/rust:tokio").is_reindeer_third_party());
        assert!(with_label("fbsource//third-party/rust/top:rustc").is_reindeer_third_party());
        assert!(
            with_label("fbsource//third-party/rust-toolchain:rust_allocator")
                .is_reindeer_third_party()
        );
        assert!(
            with_label("fbsource//third-party/rust-toolchain/1.98.1/rust:core")
                .is_reindeer_third_party()
        );

        assert!(
            !with_label("fbcode//buck2/integrations/rust-project:rust-project")
                .is_reindeer_third_party()
        );
        // A first-party crate that merely lives under a similarly-named path is
        // not a reindeer root.
        assert!(!with_label("fbcode//third-party/rust-tools/foo:foo").is_reindeer_third_party());
        assert!(
            !with_label("fbsource//third-party/rust-toolchain-tools:foo").is_reindeer_third_party()
        );
    }

    #[test]
    fn test_display_name_version() {
        let info = TargetInfo {
            name: "1.2.3".to_owned(),
            label: "//third-party/foo:1.2.3".to_owned(),
            labels: vec![],
            kind: Kind::Library,
            edition: None,
            srcs: vec![],
            mapped_srcs: FxHashMap::default(),
            crate_name: Some("foo".to_owned()),
            crate_dynamic: None,
            crate_root: PathBuf::default(),
            deps: vec![],
            test_deps: vec![],
            named_deps: FxHashMap::default(),
            proc_macro: None,
            features: vec![],
            env: FxHashMap::default(),
            source_folder: PathBuf::from("/tmp"),
            project_relative_buildfile: PathBuf::from("third-party/BUCK"),
            in_workspace: false,
            rustc_flags: vec![],
        };
        assert_eq!(info.display_name(), "foo");
    }

    #[test]
    fn test_display_name_version_with_underscore() {
        let info = TargetInfo {
            name: "_1.2.3".to_owned(),
            label: "//third-party/foo:_1.2.3".to_owned(),
            labels: vec![],
            kind: Kind::Library,
            edition: None,
            srcs: vec![],
            mapped_srcs: FxHashMap::default(),
            crate_name: Some("foo".to_owned()),
            crate_dynamic: None,
            crate_root: PathBuf::default(),
            deps: vec![],
            test_deps: vec![],
            named_deps: FxHashMap::default(),
            proc_macro: None,
            features: vec![],
            env: FxHashMap::default(),
            source_folder: PathBuf::from("/tmp"),
            project_relative_buildfile: PathBuf::from("third-party/BUCK"),
            in_workspace: false,
            rustc_flags: vec![],
        };
        assert_eq!(info.display_name(), "foo");
    }

    #[test]
    fn test_display_name_prerelease_version() {
        let info = TargetInfo {
            name: "0.3.0-pre.0".to_owned(),
            label: "//third-party/rust/kem:0.3.0-pre.0".to_owned(),
            labels: vec![],
            kind: Kind::Library,
            edition: None,
            srcs: vec![],
            mapped_srcs: FxHashMap::default(),
            crate_name: Some("kem".to_owned()),
            crate_dynamic: None,
            crate_root: PathBuf::default(),
            deps: vec![],
            test_deps: vec![],
            named_deps: FxHashMap::default(),
            proc_macro: None,
            features: vec![],
            env: FxHashMap::default(),
            source_folder: PathBuf::from("/tmp"),
            project_relative_buildfile: PathBuf::from("third-party/BUCK"),
            in_workspace: false,
            rustc_flags: vec![],
        };
        assert_eq!(info.display_name(), "kem");
    }

    #[test]
    fn test_display_name_strips_unittest_suffix() {
        let info = TargetInfo {
            name: "my_crate-unittest".to_owned(),
            label: "//foo:my_crate-unittest".to_owned(),
            labels: vec![],
            kind: Kind::Test,
            edition: None,
            srcs: vec![],
            mapped_srcs: FxHashMap::default(),
            crate_name: None,
            crate_dynamic: None,
            crate_root: PathBuf::default(),
            deps: vec![],
            test_deps: vec![],
            named_deps: FxHashMap::default(),
            proc_macro: None,
            features: vec![],
            env: FxHashMap::default(),
            source_folder: PathBuf::from("/tmp"),
            project_relative_buildfile: PathBuf::from("foo/BUCK"),
            in_workspace: false,
            rustc_flags: vec![],
        };

        assert_eq!(info.display_name(), "my_crate");
    }
}
