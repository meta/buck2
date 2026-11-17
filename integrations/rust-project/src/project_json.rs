/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A build-system independent project structure defined and used by rust-analyzer.
//!
//! While this format is provisional and subject to change, `rust-project` and Rusty
//! take a necessary dependency on these definitions to provide IDE functionality to
//! buck-based projects. For additional details, see rust-analyzer's [documentation].
//!
//! [documentation]: https://rust-analyzer.github.io/book/non_cargo_based_projects.html

use std::path::PathBuf;

use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde::Serialize;

use crate::target::Target;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectJson {
    #[serde(flatten)]
    pub(crate) sysroot: Box<Sysroot>,

    /// The set of crates comprising the project.
    ///
    /// Must include all transitive dependencies as well as sysroot crate (libstd,
    /// libcore, etc.).
    pub(crate) crates: Vec<Crate>,
    pub(crate) runnables: Vec<Runnable>,
    pub(crate) generated: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Crate {
    /// Optional crate name used for display purposes; has no semantic significance.
    pub(crate) display_name: Option<String>,
    /// The path to the root module of the crate.
    pub(crate) root_module: PathBuf,
    pub(crate) edition: Edition,
    pub(crate) deps: Vec<Dep>,
    /// Should this crate be treated as a member of
    /// current "workspace".
    ///
    /// By default, inferred from the `root_module`
    /// (members are the crates which reside inside
    /// the directory opened in the editor).
    ///
    /// Set this to `false` for things like standard
    /// library and 3rd party crates to enable
    /// performance optimizations (rust-analyzer
    /// assumes that non-member crates don't change).
    pub(crate) is_workspace_member: bool,
    /// Optionally specify the (super)set of `.rs`
    /// files comprising this crate.
    ///
    /// By default, rust-analyzer assumes that only
    /// files under `root_module.parent` can belong
    /// to a crate. `include_dirs` are included
    /// recursively, unless a subdirectory is in
    /// `exclude_dirs`.
    ///
    /// Different crates can share the same `source`.
    ///
    /// If two crates share an `.rs` file in common,
    /// they *must* have the same `source`.
    /// rust-analyzer assumes that files from one
    /// source can't refer to files in another source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<Source>,
    /// The set of cfgs activated for a given crate.
    ///
    /// With how fb imports crates into fbsource/third-party,
    /// the answer is "all of them".
    pub(crate) cfg: Vec<String>,
    /// The target triple for a given crate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) build: Option<Build>,
    /// Environment for the crate, often used by `env!`.
    pub(crate) env: FxHashMap<String, String>,
    /// Whether the crate is a proc-macro crate/
    pub(crate) is_proc_macro: bool,
    /// For proc-macro crates, path to compiled
    /// proc-macro (.so, .dylib, or .dll. depends on the platform.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) proc_macro_dylib_path: Option<PathBuf>,
    /// Working directory used when expanding proc macros invoked from this crate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) proc_macro_cwd: Option<PathBuf>,
}

/// Build system-specific additions the `rust-project.json`.
///
/// rust-analyzer encodes Cargo-specific knowledge in features
/// such as flycheck or runnable and constructs Cargo-specific commands
/// on the fly. This is a reasonable decision on its part, as most people
/// use Cargo. However, to support equivalent functionality with non-Cargo
/// build systems in rust-analyzer, this struct encodes pre-defined runnables
/// and other bits of metadata. Below is an example of `TargetSpec` in JSON:
///
/// ```json
/// "target_spec": {
///     "manifest_file": "/Users/dbarsky/fbsource/fbcode/buck2/integrations/rust-project/BUCK",
///     "target_label": "fbcode//buck2/integrations/rust-project:rust-project",
///     "target_kind": "bin",
///     "runnables": {
///         "check": [
///            "build",
///            "fbcode//buck2/integrations/rust-project:rust-project"
///         ],
///         "run": [
///             "run",
///             "fbcode//buck2/integrations/rust-project:rust-project"
///         ],
///         "test": [
///             "test",
///             "fbcode//buck2/integrations/rust-project:rust-project",
///             "--",
///             "{test_id}",
///             "--print-passing-details"
///         ]
///     },
///     "flycheck_command": [
///         "build",
///         "fbcode//buck2/integrations/rust-project:rust-project"
///     ]
/// }
/// ```
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Build {
    pub(crate) label: Target,
    /// `build_file` corresponds to the `BUCK`/`TARGETS` file.
    pub(crate) build_file: PathBuf,
    pub(crate) target_kind: TargetKind,
}

/// The target kind.
///
/// rust-analyzer defines a small set of target kinds for rust-project.json:
/// <https://github.com/rust-lang/rust-analyzer/blob/11d62122ded8349b2790977fd2cc55013cea68f1/crates/project-model/src/project_json.rs#L451>
///
/// The set of target kinds for cargo is a little bigger:
/// <https://github.com/rust-lang/rust-analyzer/blob/11d62122ded8349b2790977fd2cc55013cea68f1/crates/project-model/src/cargo_workspace.rs#L262>
/// <https://github.com/rust-lang/rust-analyzer/blob/1e6cef94dfbc359e7b1e7d392de387c2c539b965/crates/project-model/src/cargo_workspace.rs#L212>
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TargetKind {
    #[default]
    Bin,
    /// Any kind of Cargo lib crate-type (dylib, rlib, proc-macro, ...).
    Lib,
    Test,
    Other,
}

impl From<crate::target::Kind> for TargetKind {
    fn from(value: crate::target::Kind) -> Self {
        use crate::target::Kind::*;
        match value {
            Binary => TargetKind::Bin,
            Library => TargetKind::Lib,
            Test => TargetKind::Test,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Runnable {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub kind: RunnableKind,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunnableKind {
    Check,
    Flycheck,
    Run,
    TestOne,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename = "edition")]
pub(crate) enum Edition {
    #[serde(rename = "2015")]
    Edition2015,
    #[serde(rename = "2018")]
    Edition2018,
    #[default]
    #[serde(rename = "2021")]
    Edition2021,
    #[serde(rename = "2024")]
    Edition2024,
}

/// An optional set of Rust files that comprise the crate.
///
/// By default, rust-analyzer assumes that only files under
/// `Crate::root_module` can belong to a crate. `include_dirs`
/// are included recursively, unless a subdirectory is
/// specified in `include_dirs`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct Source {
    pub(crate) include_dirs: Vec<PathBuf>,
    pub(crate) exclude_dirs: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dep {
    #[serde(rename = "crate")]
    pub(crate) crate_index: usize,
    pub(crate) name: String,
}

/// Sysroot paths. These are documented in the rust-analyzer manual:
///
/// <https://rust-analyzer.github.io/book/non_cargo_based_projects.html>
///
/// rust-analyzer treats both paths as optional, but we always provide sysroot.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sysroot {
    /// Path to the directory of the sysroot; this is a superset of `sysroot_src`.
    ///
    /// This path provides rust-analyzer both the *source code* of libraries
    /// like `std` and `core` and binaries like `rust-analyzer-proc-macro-srv`,
    /// which enable rust-analyzer to expand procedural macros.
    ///
    /// For example, a `sysroot` is `~/fbsource/fbcode/third-party-buck/platform010/build/rust/`.
    ///
    /// `rust-analyzer` relies on an external binary to expand procedural
    /// macros and the source code location can be predictably inferred.
    /// Assuming the example sysroot above, the source code would be located in
    /// `/lib/rustlib/src/rust/`.
    pub(crate) sysroot: PathBuf,
    /// Legacy sysroot config containing only the source code of libraries such
    /// as `std` and core`.
    ///
    /// Inside Meta, this is necessary on non-Linux platforms since the sources
    /// are packaged separately from binaries such as `rust-analyzer-proc-macro-srv`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sysroot_src: Option<PathBuf>,
    /// A nested rust-project for the sysroot itself. If not provided, rust-analyzer
    /// will attempt to compute the sysroot layout with Cargo.
    ///
    /// Inside Meta, we have a Buck-ified rust toolchain and we can provide the
    /// sysroot layout directly with Buck.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sysroot_project: Option<ProjectJson>,
}
