/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod parser;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::LazyLock;

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use buck2_core::package::PackageLabel;
use buck2_core::package::source_path::SourcePathRef;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::ProvidersLabelMaybeConfigured;
use buck2_error::internal_error;
use buck2_util::arc_str::ArcStr;
use buck2_util::size_assert;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serializer;
use serde_json::to_value;
use smallvec::SmallVec;
use smallvec::smallvec;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::AttrWord;
use crate::attrs::arena::CoercedAttrArena;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::RefItem;
use crate::attrs::arena::Refs;
use crate::attrs::arena::fields::ChildrenOf;
use crate::attrs::arena::fields::Cursor;
use crate::attrs::arena::fields::Extent;
use crate::attrs::arena::fields::Field;
use crate::attrs::arena::fields::IdHasher;
use crate::attrs::arena::fields::RecordWriter;
use crate::attrs::arena::fields::Text;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::arena::storage::RefRun;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::attr_type::query::QueryAttrBase;
use crate::attrs::attr_type::query::QueryExprRef;
use crate::attrs::attr_type::query::QueryMacroBase;
use crate::attrs::attr_type::query::ResolvedQueryLiterals;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::kinds::CoercedAttrKind;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::JsonKey;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::coerced_path::CoercedPath;
use crate::attrs::configuration_context::AttrConfigurationContext;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::configured_traversal::ConfiguredAttrTraversal;
use crate::attrs::strong_hash_without_config::StrongHashWithoutConfig;
use crate::attrs::traversal::CoercedAttrTraversal;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum StringMacroTraversalError {
    #[error("Expected a package when traversing string macro: `{0}`.")]
    NoPackageForSource(String),
}

/// The kind of dependency for a macro like `$(location ...)` or `$(exe ...)`.
#[derive(
    Debug, Eq, PartialEq, Hash, Clone, Copy, Dupe, Allocative, Pagable, StrongHash
)]
pub enum MacroDepKind {
    /// Regular target dependency (configured with target configuration).
    Regular,
    /// Execution dependency (configured with exec configuration).
    Exec,
    /// Toolchain dependency (configured with toolchain configuration).
    Toolchain,
}

#[derive(
    Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable, StrongHash
)]
pub struct ArgAttrType {
    pub anon_target_compatible: bool,
    pub execution_group: Option<NamedExecutionGroup>,
    /// Whether the `attrs.arg()` names no execution group. `exec_group = DefaultExecGroup`
    /// names one, though `execution_group` is `None` for it as for no `exec_group`.
    pub ungrouped: bool,
}

/// [StringWithMacros] is the core representation for an attrs.arg() (in all of it's coerced, configured, and resolved
/// forms). The parsed arg string is held as a sequence of parts (each part either a literal string or a macro). When
/// being added to a command line, these parts will be concattenated together and added as a single arg.
/// Each variant takes in a boolean which determines if the resolved form should be compatible with anon targets.
#[derive(Debug, Eq, PartialEq, Hash, Clone, Allocative, Pagable, StrongHash)]
pub enum StringWithMacros<P: ProvidersLabelMaybeConfigured> {
    /// Semantically, StringWithMacros::StringPart(s) is equivalent to
    /// StringWithMacros::ManyParts(vec![StringWithMacrosPart::String(s)]). We special-case this
    /// for memory efficiency to avoid allocating unnecessary vectors, since lone string parts are
    /// very frequent.
    StringPart(ArcStr),
    // For resolution, we defer all work and simply alloc a ConfiguredStringWithMacros into the starlark
    // context. This allows us to defer resolution work to the point that it is being added to a command
    // line, but it requires that we can cheaply copy ConfiguredStringWithMacros.
    ManyParts(Box<[StringWithMacrosPart<P>]>),
}

// Avoid changing the size accidentally.
size_assert::words_of_type!(StringWithMacros<ProvidersLabel>, 3);

impl StringWithMacros<ConfiguredProvidersLabel> {
    /// Whether any macro refers to a tool (see [`MacroBase::is_tool`]).
    pub fn has_tools(&self) -> bool {
        match self {
            Self::StringPart(_) => false,
            Self::ManyParts(parts) => parts
                .iter()
                .any(|part| matches!(part, StringWithMacrosPart::Macro(_, m) if m.is_tool())),
        }
    }

    pub fn concat(
        self,
        items: impl Iterator<Item = buck2_error::Result<Self>>,
    ) -> buck2_error::Result<Self> {
        let mut parts = Vec::new();
        for x in std::iter::once(Ok(self)).chain(items) {
            match x? {
                Self::StringPart(x) => {
                    parts.push(StringWithMacrosPart::String(x));
                }
                Self::ManyParts(xs) => {
                    parts.extend(xs.into_vec());
                }
            }
        }
        Ok(Self::ManyParts(parts.into_boxed_slice()))
    }
}

impl StrongHashWithoutConfig for StringWithMacros<ConfiguredProvidersLabel> {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::StringPart(part) => {
                "StringPart".strong_hash(state);
                part.strong_hash(state);
            }
            Self::ManyParts(parts) => {
                "ManyParts".strong_hash(state);
                (parts.len() as u64).strong_hash(state);
                parts
                    .iter()
                    .for_each(|part| part.strong_hash_without_config(state));
            }
        }
    }
}

impl StringWithMacros<ConfiguredProvidersLabel> {
    pub fn traverse(
        &self,
        traversal: &mut dyn ConfiguredAttrTraversal,
        pkg: PackageLabel,
    ) -> buck2_error::Result<()> {
        match self {
            Self::StringPart(..) => {}
            Self::ManyParts(parts) => {
                for part in parts.iter() {
                    match part {
                        StringWithMacrosPart::String(_) => {}
                        StringWithMacrosPart::Macro(_, m) => {
                            m.traverse(traversal, pkg)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq, Hash, Clone, Allocative, Pagable, StrongHash)]
pub enum StringWithMacrosPart<P: ProvidersLabelMaybeConfigured> {
    String(ArcStr),
    Macro(/* write_to_file */ bool, MacroBase<P>),
}

impl StrongHashWithoutConfig for StringWithMacrosPart<ConfiguredProvidersLabel> {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::String(value) => {
                "String".strong_hash(state);
                value.strong_hash(state);
            }
            Self::Macro(write_to_file, macro_) => {
                "Macro".strong_hash(state);
                write_to_file.strong_hash(state);
                macro_.strong_hash_without_config(state);
            }
        }
    }
}

size_assert::words_of_type!(MacroBase<ProvidersLabel>, 3);
size_assert::words_of_type!(StringWithMacrosPart<ProvidersLabel>, 4);

#[derive(Debug, Eq, PartialEq, Hash, Clone, Allocative, Pagable, StrongHash)]
pub enum MacroBase<P: ProvidersLabelMaybeConfigured> {
    Location {
        label: P,
        dep_kind: MacroDepKind,
    },
    /// Represents both $(exe) and $(exe_target) usages.
    Exe {
        label: P,
        exec_dep: bool,
    },
    /// A user-defined make variable (like `$(CXX)`). This will be resolved based on the propagated TemplateVariableInfos.
    UserUnkeyedPlaceholder(Box<str>),

    /// A user-defined macro (like `$(cxxppflags //some:target)`). This will be resolved based on the propagated TemplateVariableInfos.
    UserKeyedPlaceholder(Box<(Box<str>, P, Option<Box<str>>)>),

    Query(Box<QueryMacroBase<P>>),
    Source(CoercedPath),

    /// A `$(exe)`, `$(location_exec)` or `$(location_toolchain)` macro from an arg attribute in a
    /// named execution group: its dependency is configured for, and constrains, that group.
    InGroup(Box<(MacroBase<P>, NamedExecutionGroup)>),
}

impl StrongHashWithoutConfig for MacroBase<ConfiguredProvidersLabel> {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Location { label, dep_kind } => {
                "Location".strong_hash(state);
                label.strong_hash_without_config(state);
                dep_kind.strong_hash(state);
            }
            Self::Exe { label, exec_dep } => {
                "Exe".strong_hash(state);
                label.strong_hash_without_config(state);
                exec_dep.strong_hash(state);
            }
            Self::UserUnkeyedPlaceholder(name) => {
                "UserUnkeyedPlaceholder".strong_hash(state);
                name.strong_hash(state);
            }
            Self::UserKeyedPlaceholder(value) => {
                "UserKeyedPlaceholder".strong_hash(state);
                let (name, label, arg) = value.as_ref();
                name.strong_hash(state);
                label.strong_hash_without_config(state);
                arg.strong_hash(state);
            }
            Self::Query(query) => {
                "Query".strong_hash(state);
                query.strong_hash_without_config(state);
            }
            Self::Source(path) => {
                "Source".strong_hash(state);
                path.strong_hash(state);
            }
            Self::InGroup(box (inner, group)) => {
                "InGroup".strong_hash(state);
                inner.strong_hash_without_config(state);
                group.strong_hash(state);
            }
        }
    }
}

impl MacroBase<ConfiguredProvidersLabel> {
    pub fn traverse(
        &self,
        traversal: &mut dyn ConfiguredAttrTraversal,
        pkg: PackageLabel,
    ) -> buck2_error::Result<()> {
        // macros can't reference repo inputs (they only reference the outputs of other targets)
        match self {
            MacroBase::UserKeyedPlaceholder(box (_, l, _)) => traversal.dep(l),
            MacroBase::Location {
                label,
                dep_kind: MacroDepKind::Exec,
            } => traversal.exec_dep(label),
            MacroBase::Location {
                label,
                dep_kind: MacroDepKind::Toolchain,
            } => traversal.toolchain_dep(label),
            MacroBase::Location {
                label,
                dep_kind: MacroDepKind::Regular,
            } => traversal.dep(label),
            MacroBase::Exe {
                label,
                exec_dep: true,
            } => traversal.exec_dep(label),
            MacroBase::Exe {
                label,
                exec_dep: false,
            } => traversal.dep(label),
            MacroBase::Source(path) => {
                for x in path.inputs() {
                    traversal.input(SourcePathRef::new(pkg.dupe(), x))?;
                }
                Ok(())
            }
            MacroBase::Query(query_macro) => query_macro.traverse(traversal),
            MacroBase::UserUnkeyedPlaceholder(_) => Ok(()),
            MacroBase::InGroup(box (inner, group)) => match inner {
                MacroBase::Location {
                    label,
                    dep_kind: MacroDepKind::Exec,
                }
                | MacroBase::Exe {
                    label,
                    exec_dep: true,
                } => traversal.exec_dep_in_group(label, group),
                MacroBase::Location {
                    label,
                    dep_kind: MacroDepKind::Toolchain,
                } => traversal.toolchain_dep_in_group(label, group),
                _ => inner.traverse(traversal, pkg),
            },
        }
    }
}

impl MacroBase<ProvidersLabel> {
    pub fn configure(
        &self,
        ctx: &dyn AttrConfigurationContext,
        execution_group: Option<&NamedExecutionGroup>,
    ) -> buck2_error::Result<ConfiguredMacro> {
        if let Some(group) = execution_group {
            let in_group = match self {
                UnconfiguredMacro::Location {
                    label,
                    dep_kind: MacroDepKind::Exec,
                } => Some(ConfiguredMacro::Location {
                    label: ctx.configure_exec_target_in_group(label, group)?,
                    dep_kind: MacroDepKind::Exec,
                }),
                UnconfiguredMacro::Location {
                    label,
                    dep_kind: MacroDepKind::Toolchain,
                } => Some(ConfiguredMacro::Location {
                    label: ctx.configure_toolchain_target_in_group(label, group)?,
                    dep_kind: MacroDepKind::Toolchain,
                }),
                UnconfiguredMacro::Exe {
                    label,
                    exec_dep: true,
                } => Some(ConfiguredMacro::Exe {
                    label: ctx.configure_exec_target_in_group(label, group)?,
                    exec_dep: true,
                }),
                _ => None,
            };
            if let Some(configured) = in_group {
                return Ok(ConfiguredMacro::InGroup(Box::new((
                    configured,
                    group.dupe(),
                ))));
            }
        }
        Ok(match self {
            UnconfiguredMacro::Location { label, dep_kind } => ConfiguredMacro::Location {
                label: match dep_kind {
                    MacroDepKind::Exec => ctx.configure_exec_target(label)?,
                    MacroDepKind::Toolchain => ctx.configure_toolchain_target(label),
                    MacroDepKind::Regular => ctx.configure_target(label),
                },
                dep_kind: *dep_kind,
            },
            UnconfiguredMacro::Exe { label, exec_dep } => ConfiguredMacro::Exe {
                label: if *exec_dep {
                    ctx.configure_exec_target(label)?
                } else {
                    ctx.configure_target(label)
                },
                exec_dep: *exec_dep,
            },
            UnconfiguredMacro::UserUnkeyedPlaceholder(var_name) => {
                ConfiguredMacro::UserUnkeyedPlaceholder(var_name.clone())
            }
            UnconfiguredMacro::UserKeyedPlaceholder(box (var_name, target, arg)) => {
                ConfiguredMacro::UserKeyedPlaceholder(Box::new((
                    var_name.clone(),
                    ctx.configure_target(target),
                    arg.clone(),
                )))
            }
            UnconfiguredMacro::Query(query) => {
                ConfiguredMacro::Query(Box::new(query.configure(ctx)?))
            }
            UnconfiguredMacro::Source(path) => ConfiguredMacro::Source(path.clone()),
            UnconfiguredMacro::InGroup(box (inner, group)) => {
                return inner.configure(ctx, Some(group));
            }
        })
    }

    pub fn traverse<'a>(
        &'a self,
        traversal: &mut dyn CoercedAttrTraversal<'a>,
        pkg: Option<PackageLabel>,
    ) -> buck2_error::Result<()> {
        match self {
            MacroBase::UserKeyedPlaceholder(box (_, l, _)) => traversal.dep(l),
            MacroBase::Location {
                label,
                dep_kind: MacroDepKind::Exec,
            } => traversal.exec_dep(label),
            MacroBase::Location {
                label,
                dep_kind: MacroDepKind::Toolchain,
            } => traversal.toolchain_dep(label),
            MacroBase::Location {
                label,
                dep_kind: MacroDepKind::Regular,
            } => traversal.dep(label),
            MacroBase::Exe {
                label,
                exec_dep: true,
            } => traversal.exec_dep(label),
            MacroBase::Exe {
                label,
                exec_dep: false,
            } => traversal.dep(label),
            MacroBase::Query(query) => query.traverse(traversal),
            MacroBase::Source(path) => {
                let pkg = pkg.ok_or_else(|| {
                    StringMacroTraversalError::NoPackageForSource(path.path().to_string())
                })?;
                for x in path.inputs() {
                    traversal.input(SourcePathRef::new(pkg.dupe(), x))?;
                }
                Ok(())
            }
            MacroBase::UserUnkeyedPlaceholder(_) => Ok(()),
            MacroBase::InGroup(box (inner, _)) => inner.traverse(traversal, pkg),
        }
    }
}

// These type aliases are just a little bit easier to use, the differentiating thing comes
// right at the beginning instead of at the end in a type param.
pub type UnconfiguredMacro = MacroBase<ProvidersLabel>;
pub type ConfiguredMacro = MacroBase<ConfiguredProvidersLabel>;

pub type UnconfiguredStringWithMacrosPart = StringWithMacrosPart<ProvidersLabel>;
pub type ConfiguredStringWithMacrosPart = StringWithMacrosPart<ConfiguredProvidersLabel>;

pub type UnconfiguredStringWithMacros = StringWithMacros<ProvidersLabel>;

#[derive(Debug, Clone, Allocative, Display, Pagable)]
#[display("{}", string_with_macros)]
pub struct ConfiguredStringWithMacros {
    pub string_with_macros: StringWithMacros<ConfiguredProvidersLabel>,
    /// `None` for an arg that cannot be passed to anon targets. For one that can, the execution
    /// platform resolution its tools (see [`MacroBase::is_tool`]) were configured for, which is
    /// that of the target that configured it; `Some(None)`, shared through [`NO_TOOLS`] rather
    /// than allocated, when it refers to no tools. One pointer, so `ConfiguredAttr` keeps its size.
    anon_target: Option<Arc<Option<ExecutionPlatformResolution>>>,
}

static NO_TOOLS: LazyLock<Arc<Option<ExecutionPlatformResolution>>> =
    LazyLock::new(|| Arc::new(None));

impl ConfiguredStringWithMacros {
    /// `configured_by` is the execution platform resolution of the target that configured the
    /// arg, if any; it is kept only for an anon-target-compatible arg that refers to tools.
    pub fn new(
        string_with_macros: StringWithMacros<ConfiguredProvidersLabel>,
        anon_target_compatible: bool,
        configured_by: Option<&ExecutionPlatformResolution>,
    ) -> Self {
        let anon_target = if !anon_target_compatible {
            None
        } else if string_with_macros.has_tools() {
            Some(Arc::new(Some(configured_by.map_or_else(
                ExecutionPlatformResolution::unspecified,
                Dupe::dupe,
            ))))
        } else {
            Some(NO_TOOLS.dupe())
        };
        Self {
            string_with_macros,
            anon_target,
        }
    }

    pub fn anon_target_compatible(&self) -> bool {
        self.anon_target.is_some()
    }

    /// The execution platform resolution the tools of an anon-target-compatible arg were
    /// configured for; `None` if it refers to no tools.
    pub fn anon_target_tool_resolution(&self) -> Option<&ExecutionPlatformResolution> {
        self.anon_target.as_deref()?.as_ref()
    }
}

/// The resolution in `anon_target` is left out of equality and of every hash, so
/// anon targets created by targets with different resolutions from the same arg are one target.
impl PartialEq for ConfiguredStringWithMacros {
    fn eq(&self, other: &Self) -> bool {
        self.string_with_macros == other.string_with_macros
            && self.anon_target_compatible() == other.anon_target_compatible()
    }
}

impl Eq for ConfiguredStringWithMacros {}

impl Hash for ConfiguredStringWithMacros {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.string_with_macros.hash(state);
        self.anon_target_compatible().hash(state);
    }
}

impl StrongHash for ConfiguredStringWithMacros {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.string_with_macros.strong_hash(state);
        self.anon_target_compatible().strong_hash(state);
    }
}

impl StrongHashWithoutConfig for ConfiguredStringWithMacros {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        self.string_with_macros.strong_hash_without_config(state);
        self.anon_target_compatible().strong_hash(state);
    }
}

impl<P: ProvidersLabelMaybeConfigured> MacroBase<P> {
    /// A tool is the dependency of `$(exe)` on an exec dep, `$(location_exec)`,
    /// `$(location_toolchain)`, or any macro of an arg in an execution group.
    pub fn is_tool(&self) -> bool {
        matches!(
            self,
            Self::InGroup(..)
                | Self::Exe { exec_dep: true, .. }
                | Self::Location {
                    dep_kind: MacroDepKind::Exec | MacroDepKind::Toolchain,
                    ..
                }
        )
    }
}

/// Display attempts to approximately reproduce the string that created a macro.
impl<P: ProvidersLabelMaybeConfigured> Display for MacroBase<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO: this should re-escape values in the args that need to be escaped to have returned that arg (it's not possible
        // to tell where there were unnecessary escapes and it's not worth tracking that).
        match self {
            MacroBase::Location { label, dep_kind } => {
                write!(
                    f,
                    "{} {}",
                    match dep_kind {
                        MacroDepKind::Exec => "location_exec",
                        MacroDepKind::Toolchain => "location_toolchain",
                        MacroDepKind::Regular => "location",
                    },
                    label
                )
            }
            MacroBase::Exe { label, exec_dep } => {
                write!(
                    f,
                    "{} {}",
                    if *exec_dep { "exe" } else { "exe_target" },
                    label
                )
            }
            MacroBase::Query(query) => Display::fmt(query, f),
            MacroBase::Source(path) => write!(f, "src {}", path.path()),
            MacroBase::InGroup(box (inner, _)) => Display::fmt(inner, f),
            MacroBase::UserUnkeyedPlaceholder(var) => write!(f, "{var}"),
            MacroBase::UserKeyedPlaceholder(box (macro_type, target, arg)) => write!(
                f,
                "{} {}{}",
                macro_type,
                target,
                if let Some(arg) = arg {
                    format!(" {arg}")
                } else {
                    "".to_owned()
                }
            ),
        }
    }
}

impl<P: ProvidersLabelMaybeConfigured> Display for StringWithMacrosPart<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StringWithMacrosPart::String(s) => write!(f, "{s}"),
            StringWithMacrosPart::Macro(write_to_file, m) => {
                write!(f, "$({}{})", if *write_to_file { "@" } else { "" }, m)
            }
        }
    }
}

impl<P: ProvidersLabelMaybeConfigured> Display for StringWithMacros<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StringPart(part) => {
                write!(f, "{part}")?;
            }
            Self::ManyParts(parts) => {
                for part in parts.iter() {
                    write!(f, "{part}")?;
                }
            }
        }

        Ok(())
    }
}

/// Represents the type of a query placeholder (e.g. query_outputs, query_targets, query_targets_and_outputs).
#[derive(Debug, Eq, PartialEq, Hash, Clone, Allocative, Pagable, StrongHash)]
pub enum QueryExpansion {
    Output,
    Target,
    /// Holds the separator used between a target and its output. If not provided, they will be space-separated.
    TargetAndOutput(Option<String>),
}

impl Display for QueryExpansion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryExpansion::Output => f.write_str("query_outputs")?,
            QueryExpansion::Target => f.write_str("query_targets")?,
            QueryExpansion::TargetAndOutput(Some(separator)) => {
                write!(f, "query_targets_and_outputs '{separator}'")?;
            }
            QueryExpansion::TargetAndOutput(None) => f.write_str("query_targets_and_outputs")?,
        };

        Ok(())
    }
}

impl<'a> ArgPartView<'a> {
    /// Mirror of `MacroBase::<ProvidersLabel>::configure` off the view.
    pub(crate) fn configure(
        &self,
        ctx: &dyn AttrConfigurationContext,
        execution_group: Option<&NamedExecutionGroup>,
    ) -> buck2_error::Result<ConfiguredStringWithMacrosPart> {
        Ok(match self {
            ArgPartView::String(s) => StringWithMacrosPart::String(ArcStr::from(*s)),
            ArgPartView::Macro { write_to_file, mac } => StringWithMacrosPart::Macro(
                *write_to_file,
                match (mac, execution_group) {
                    (
                        ArgMacroView::Location {
                            label,
                            dep_kind: MacroDepKind::Exec,
                        },
                        Some(group),
                    ) => ConfiguredMacro::InGroup(Box::new((
                        ConfiguredMacro::Location {
                            label: ctx.configure_exec_target_in_group(label, group)?,
                            dep_kind: MacroDepKind::Exec,
                        },
                        group.dupe(),
                    ))),
                    (
                        ArgMacroView::Location {
                            label,
                            dep_kind: MacroDepKind::Toolchain,
                        },
                        Some(group),
                    ) => ConfiguredMacro::InGroup(Box::new((
                        ConfiguredMacro::Location {
                            label: ctx.configure_toolchain_target_in_group(label, group)?,
                            dep_kind: MacroDepKind::Toolchain,
                        },
                        group.dupe(),
                    ))),
                    (
                        ArgMacroView::Exe {
                            label,
                            exec_dep: true,
                        },
                        Some(group),
                    ) => ConfiguredMacro::InGroup(Box::new((
                        ConfiguredMacro::Exe {
                            label: ctx.configure_exec_target_in_group(label, group)?,
                            exec_dep: true,
                        },
                        group.dupe(),
                    ))),
                    (mac, _) => match mac {
                        ArgMacroView::Location { label, dep_kind } => ConfiguredMacro::Location {
                            label: match dep_kind {
                                MacroDepKind::Exec => ctx.configure_exec_target(label)?,
                                MacroDepKind::Toolchain => ctx.configure_toolchain_target(label),
                                MacroDepKind::Regular => ctx.configure_target(label),
                            },
                            dep_kind: *dep_kind,
                        },
                        ArgMacroView::Exe { label, exec_dep } => ConfiguredMacro::Exe {
                            label: if *exec_dep {
                                ctx.configure_exec_target(label)?
                            } else {
                                ctx.configure_target(label)
                            },
                            exec_dep: *exec_dep,
                        },
                        ArgMacroView::UserUnkeyedPlaceholder(name) => {
                            ConfiguredMacro::UserUnkeyedPlaceholder(Box::from(*name))
                        }
                        ArgMacroView::UserKeyedPlaceholder { var, label, arg } => {
                            ConfiguredMacro::UserKeyedPlaceholder(Box::new((
                                Box::from(*var),
                                ctx.configure_target(label),
                                arg.map(Box::from),
                            )))
                        }
                        ArgMacroView::Source(path) => ConfiguredMacro::Source((*path).clone()),
                        ArgMacroView::Query(q) => {
                            ConfiguredMacro::Query(Box::new(q.configure(ctx)))
                        }
                    },
                },
            ),
        })
    }
}

impl Kind for kinds::ArgSimple {
    type Fields = Text;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_arg_simple(&mut self, s: &str) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::ArgSimple>(self, s)
    }
}

impl Semantics for kinds::ArgSimple {
    const CONCAT: Option<&'static str> = Some("arg");

    fn traverse<'a>(_: View<'a, Self>, _: Traverse<'_, 'a>) -> buck2_error::Result<()> {
        Ok(())
    }

    fn configure(
        s: View<'_, Self>,
        Configure { v, t, ctx, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Arg(ty) => ResultMaybeCompatible::Compatible(ConfiguredAttr::Arg(
                ConfiguredStringWithMacros::new(
                    StringWithMacros::StringPart(ArcStr::from(s)),
                    ty.anon_target_compatible,
                    ctx.execution_platform_resolution(),
                ),
            )),
            _ => mismatch(v, t).into(),
        }
    }

    fn traverse_configured(
        _: View<'_, Self>,
        _: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(s: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        Ok(to_value(s)?)
    }

    fn fmt(s: View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "\"{s}\"")
    }

    fn any_matches(
        s: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        filter(s)
    }

    fn json_key<'a>(s: View<'a, Self>, _: JsonKey<'_>) -> buck2_error::Result<Cow<'a, str>> {
        Ok(Cow::Borrowed(s))
    }

    fn serialize<S: Serializer>(
        s: View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_str(s)
    }
}

impl Kind for kinds::Arg {
    type Fields = ChildrenOf<ArgParts>;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_arg_parts(&mut self, parts: &[AttrToken]) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::Arg>(self, parts)
    }
}

impl Semantics for kinds::Arg {
    const CONCAT: Option<&'static str> = Some("arg");

    fn traverse<'a>(
        parts: View<'a, Self>,
        Traverse { v, t, pkg, tr }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        let AttrTypeInner::Arg(ty) = &t.non_optional().0.inner else {
            return Err(mismatch(v, t));
        };
        for part in parts.iter() {
            let ArgPartView::Macro { mac, .. } = part else {
                continue;
            };
            if ty.ungrouped
                && matches!(
                    mac,
                    ArgMacroView::Exe { exec_dep: true, .. }
                        | ArgMacroView::Location {
                            dep_kind: MacroDepKind::Exec | MacroDepKind::Toolchain,
                            ..
                        }
                )
            {
                tr.ungrouped_tool_macro(&mac)?;
            }
            match mac {
                ArgMacroView::UserKeyedPlaceholder { label, .. } => tr.dep(label)?,
                ArgMacroView::Location {
                    label,
                    dep_kind: MacroDepKind::Exec,
                } => tr.exec_dep(label)?,
                ArgMacroView::Location {
                    label,
                    dep_kind: MacroDepKind::Toolchain,
                } => tr.toolchain_dep(label)?,
                ArgMacroView::Location {
                    label,
                    dep_kind: MacroDepKind::Regular,
                } => tr.dep(label)?,
                ArgMacroView::Exe {
                    label,
                    exec_dep: true,
                } => tr.exec_dep(label)?,
                ArgMacroView::Exe {
                    label,
                    exec_dep: false,
                } => tr.dep(label)?,
                ArgMacroView::Source(path) => {
                    let pkg = pkg.ok_or_else(|| {
                        internal_error!("Expected a package when traversing arg source macro")
                    })?;
                    for x in path.inputs() {
                        tr.input(SourcePathRef::new(pkg, x))?;
                    }
                }
                // Literal labels only, in expression order.
                ArgMacroView::Query(q) => {
                    let mut labels = Vec::new();
                    q.literal_labels(&mut labels);
                    for label in labels {
                        tr.dep(label)?;
                    }
                }
                ArgMacroView::UserUnkeyedPlaceholder(_) => {}
            }
        }
        Ok(())
    }

    fn configure(
        parts: View<'_, Self>,
        Configure { v, t, ctx, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Arg(ty) => {
                let configured = parts
                    .iter()
                    .map(|p| p.configure(ctx, ty.execution_group.as_ref()))
                    .collect::<buck2_error::Result<Box<[_]>>>()?;
                ResultMaybeCompatible::Compatible(ConfiguredAttr::Arg(
                    ConfiguredStringWithMacros::new(
                        StringWithMacros::ManyParts(configured),
                        ty.anon_target_compatible,
                        ctx.execution_platform_resolution(),
                    ),
                ))
            }
            _ => mismatch(v, t).into(),
        }
    }

    fn to_json(parts: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        Ok(to_value(parts.to_string())?)
    }

    fn fmt(parts: View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "\"{parts}\"")
    }

    fn any_matches(
        parts: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        filter(&parts.to_string())
    }

    fn json_key<'a>(parts: View<'a, Self>, _: JsonKey<'_>) -> buck2_error::Result<Cow<'a, str>> {
        Ok(Cow::Owned(parts.to_string()))
    }

    fn serialize<S: Serializer>(
        parts: View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&parts.to_string())
    }
}

/// Arg macro record header (its first head field):
/// - bits 0-3: variant tag (`ARG_MACRO_TAG_*`)
/// - bits 4-11: variant flag byte — location: `MacroDepKind`; exe:
///   `exec_dep`; keyed placeholder: has-arg; query: expansion kind in
///   flag bits 0-1, has-separator in flag bit 2
/// - bit 12: `write_to_file`
const ARG_MACRO_TAG_MASK: u32 = 0xF;
const ARG_MACRO_FLAG_MASK: u32 = 0xFF;
const ARG_MACRO_TAG_LOCATION: u32 = 0;
const ARG_MACRO_TAG_EXE: u32 = 1;
const ARG_MACRO_TAG_UNKEYED: u32 = 2;
const ARG_MACRO_TAG_KEYED: u32 = 3;
const ARG_MACRO_TAG_SOURCE: u32 = 4;
/// Refs: `[expr, sep]`; `expr` is the parsed expression as a `QueryExpr`
/// record, `sep` a text record present when the has-separator flag bit is
/// set.
const ARG_MACRO_TAG_QUERY: u32 = 5;
const ARG_MACRO_FLAG_SHIFT: u32 = 4;
const ARG_MACRO_WTF_BIT: u32 = 1 << 12;

const ARG_QUERY_EXPANSION_MASK: u32 = 0x3;
const ARG_QUERY_EXPANSION_OUTPUT: u32 = 0;
const ARG_QUERY_EXPANSION_TARGET: u32 = 1;
const ARG_QUERY_EXPANSION_TARGET_AND_OUTPUT: u32 = 2;
const ARG_QUERY_HAS_SEP: u32 = 4;

fn dep_kind_bits(k: MacroDepKind) -> u32 {
    match k {
        MacroDepKind::Regular => 0,
        MacroDepKind::Exec => 1,
        MacroDepKind::Toolchain => 2,
    }
}

fn dep_kind_from_bits(bits: u32) -> MacroDepKind {
    match bits {
        0 => MacroDepKind::Regular,
        1 => MacroDepKind::Exec,
        2 => MacroDepKind::Toolchain,
        _ => unreachable!("should be valid: records are only written by the encoder"),
    }
}

/// Header, payload index, and refs of an `ArgMacro` record.
fn macro_parts(arena: &CoercedAttrArena, r: AttrWord) -> (u32, u32, RefRun<'_>) {
    let mut c = Cursor::new(arena, r);
    let header = c.raw();
    let payload = c.raw();
    let n = c.raw();
    (header, payload, c.refs(n))
}

impl<'a> ArgPartView<'a> {
    /// The part at `r`: an `ArgSimple` text record or an `ArgMacro` record.
    fn decode(arena: &'a CoercedAttrArena, r: AttrWord) -> ArgPartView<'a> {
        match arena.kind_of(r) {
            CoercedAttrKind::ArgSimple => ArgPartView::String(arena.str_at(r)),
            CoercedAttrKind::ArgMacro => {
                let (write_to_file, mac) = <kinds::ArgMacro as Kind>::decode(arena, r);
                ArgPartView::Macro { write_to_file, mac }
            }
            _ => unreachable!("should be valid: arg parts are ArgSimple or ArgMacro refs"),
        }
    }
}

/// One arg part (`ArgSimple` or `ArgMacro` record) per ref.
pub struct ArgParts;

impl RefItem for ArgParts {
    const STRIDE: u32 = 1;
    type Item<'a> = ArgPartView<'a>;
    type Spec = [AttrToken];
    fn item<'a>(refs: Refs<'a, Self>, i: u32) -> ArgPartView<'a> {
        ArgPartView::decode(refs.run.arena, refs.run.get(i))
    }
    fn tokens(spec: &[AttrToken]) -> impl Iterator<Item = AttrToken> + '_ {
        spec.iter().copied()
    }
}

/// One arg macro record: head `[header, payload, n]` then `n` refs, laid
/// out per the `ARG_MACRO_*` constants; the view is the `write_to_file`
/// flag and the macro.
pub struct Macro;

impl Field for Macro {
    const DEPS: bool = true;
    const INLINE: bool = false;
    const HAS_TEXT: bool = false;
    type View<'a> = (bool, ArgMacroView<'a>);
    type Spec<'s> = (bool, MacroParts<'s>);

    fn shape(peek: &dyn Fn(u32) -> Option<u32>, at: u32) -> Option<Extent> {
        Some(Extent {
            head: 3,
            refs: Some(peek(at + 2)?),
            text: None,
        })
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> (bool, ArgMacroView<'a>) {
        let arena = c.arena;
        let r = c.r;
        let header = c.raw();
        let payload = c.raw();
        let n = c.raw();
        let refs = c.refs(n);
        let flag = (header >> ARG_MACRO_FLAG_SHIFT) & ARG_MACRO_FLAG_MASK;
        let mac = match header & ARG_MACRO_TAG_MASK {
            ARG_MACRO_TAG_LOCATION => ArgMacroView::Location {
                label: &arena.providers_labels[payload as usize],
                dep_kind: dep_kind_from_bits(flag),
            },
            ARG_MACRO_TAG_EXE => ArgMacroView::Exe {
                label: &arena.providers_labels[payload as usize],
                exec_dep: flag != 0,
            },
            ARG_MACRO_TAG_UNKEYED => {
                ArgMacroView::UserUnkeyedPlaceholder(arena.str_at(refs.get(0)))
            }
            ARG_MACRO_TAG_KEYED => ArgMacroView::UserKeyedPlaceholder {
                var: arena.str_at(refs.get(0)),
                label: &arena.providers_labels[payload as usize],
                arg: (flag != 0).then(|| arena.str_at(refs.get(1))),
            },
            ARG_MACRO_TAG_SOURCE => ArgMacroView::Source(&arena.paths[payload as usize]),
            ARG_MACRO_TAG_QUERY => ArgMacroView::Query(ArgQueryView { arena, r }),
            _ => unreachable!("should be valid: records are only written by the encoder"),
        };
        (header & ARG_MACRO_WTF_BIT != 0, mac)
    }

    fn encode<'s>(
        (write_to_file, mac): (bool, MacroParts<'s>),
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        write_macro(b, w, write_to_file, mac)
    }

    /// The same record and hash as [`Self::encode`] from the parsed macro,
    /// so copies dedup against parsed originals.
    fn reemit<'a>(
        (write_to_file, mac): (bool, ArgMacroView<'a>),
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        let parts = match mac {
            ArgMacroView::Location { label, dep_kind } => MacroParts::Location { label, dep_kind },
            ArgMacroView::Exe { label, exec_dep } => MacroParts::Exe { label, exec_dep },
            ArgMacroView::UserUnkeyedPlaceholder(name) => MacroParts::UserUnkeyedPlaceholder(name),
            ArgMacroView::UserKeyedPlaceholder { var, label, arg } => {
                MacroParts::UserKeyedPlaceholder { var, label, arg }
            }
            ArgMacroView::Source(path) => MacroParts::Source(path),
            ArgMacroView::Query(q) => {
                let expr = b.encode_query_expr_view(q.expr())?;
                let (expansion, sep) = q.expansion_parts();
                MacroParts::Query {
                    expr,
                    expansion,
                    sep,
                }
            }
        };
        write_macro(b, w, write_to_file, parts)
    }

    fn identity((write_to_file, mac): (bool, ArgMacroView<'_>), h: &mut IdHasher<'_>) {
        let h = &mut h.h;
        write_to_file.strong_hash(h);
        match mac {
            ArgMacroView::Location { label, dep_kind } => {
                h.write_u8(ARG_MACRO_TAG_LOCATION as u8);
                h.write_u8(dep_kind_bits(dep_kind) as u8);
                label.strong_hash(h);
            }
            ArgMacroView::Exe { label, exec_dep } => {
                h.write_u8(ARG_MACRO_TAG_EXE as u8);
                exec_dep.strong_hash(h);
                label.strong_hash(h);
            }
            ArgMacroView::UserUnkeyedPlaceholder(var) => {
                h.write_u8(ARG_MACRO_TAG_UNKEYED as u8);
                var.strong_hash(h);
            }
            ArgMacroView::UserKeyedPlaceholder { var, label, arg } => {
                h.write_u8(ARG_MACRO_TAG_KEYED as u8);
                var.strong_hash(h);
                label.strong_hash(h);
                arg.strong_hash(h);
            }
            ArgMacroView::Source(path) => {
                h.write_u8(ARG_MACRO_TAG_SOURCE as u8);
                path.strong_hash(h);
            }
            ArgMacroView::Query(q) => {
                h.write_u8(ARG_MACRO_TAG_QUERY as u8);
                let (kind, sep) = q.expansion_parts();
                h.write_u8(kind as u8);
                sep.strong_hash(h);
                q.expr().strong_hash(h);
            }
        }
    }

    fn same(a: (bool, ArgMacroView<'_>), b: (bool, ArgMacroView<'_>)) -> bool {
        a == b
    }
}

impl Kind for kinds::ArgMacro {
    type Fields = Macro;
}

/// One decoded part of an `Arg` value containing macros.
#[derive(Copy, Clone, PartialEq)]
pub enum ArgPartView<'a> {
    String(&'a str),
    Macro {
        write_to_file: bool,
        mac: ArgMacroView<'a>,
    },
}

/// One decoded arg macro record.
#[derive(Copy, Clone, PartialEq)]
pub enum ArgMacroView<'a> {
    Location {
        label: &'a ProvidersLabel,
        dep_kind: MacroDepKind,
    },
    Exe {
        label: &'a ProvidersLabel,
        exec_dep: bool,
    },
    UserUnkeyedPlaceholder(&'a str),
    UserKeyedPlaceholder {
        var: &'a str,
        label: &'a ProvidersLabel,
        arg: Option<&'a str>,
    },
    Source(&'a CoercedPath),
    Query(ArgQueryView<'a>),
}

/// Mirrors `Display for MacroBase`: approximately the source text of the
/// macro, without the `$(...)` wrapper.
impl std::fmt::Display for ArgMacroView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgMacroView::Location { label, dep_kind } => {
                write!(
                    f,
                    "{} {}",
                    match dep_kind {
                        MacroDepKind::Exec => "location_exec",
                        MacroDepKind::Toolchain => "location_toolchain",
                        MacroDepKind::Regular => "location",
                    },
                    label
                )
            }
            ArgMacroView::Exe { label, exec_dep } => {
                write!(
                    f,
                    "{} {}",
                    if *exec_dep { "exe" } else { "exe_target" },
                    label
                )
            }
            ArgMacroView::UserUnkeyedPlaceholder(var) => write!(f, "{var}"),
            ArgMacroView::UserKeyedPlaceholder { var, label, arg } => {
                write!(f, "{var} {label}")?;
                if let Some(arg) = arg {
                    write!(f, " {arg}")?;
                }
                Ok(())
            }
            ArgMacroView::Source(path) => write!(f, "src {}", path.path()),
            ArgMacroView::Query(q) => write!(f, "{} {}", q.expansion_type(), q.query().0),
        }
    }
}

/// Mirrors `Display for StringWithMacros`: the parts concatenated, string
/// parts raw and macro parts as `$(...)`, without surrounding quotes.
impl std::fmt::Display for Refs<'_, ArgParts> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for part in self.iter() {
            match part {
                ArgPartView::String(s) => write!(f, "{s}")?,
                ArgPartView::Macro { write_to_file, mac } => {
                    write!(f, "$({}{})", if write_to_file { "@" } else { "" }, mac)?
                }
            }
        }
        Ok(())
    }
}

/// Decoded view of a query macro record (`$(query_targets ...)` etc.).
#[derive(Copy, Clone)]
pub struct ArgQueryView<'a> {
    arena: &'a CoercedAttrArena,
    r: AttrWord,
}

impl<'a> ArgQueryView<'a> {
    fn flags(&self) -> u32 {
        let (header, _, _) = macro_parts(self.arena, self.r);
        (header >> ARG_MACRO_FLAG_SHIFT) & ARG_MACRO_FLAG_MASK
    }

    pub(crate) fn expr(&self) -> QueryExprRef<'a> {
        let (_, _, refs) = macro_parts(self.arena, self.r);
        QueryExprRef {
            arena: self.arena,
            r: refs.get(0),
        }
    }

    fn separator(&self) -> Option<&'a str> {
        let (_, _, refs) = macro_parts(self.arena, self.r);
        (self.flags() & ARG_QUERY_HAS_SEP != 0).then(|| self.arena.str_at(refs.get(1)))
    }

    /// Expansion kind bits and the borrowed separator, for readers that must
    /// not allocate; the owned [`QueryExpansion`] clones the separator.
    fn expansion_parts(&self) -> (u32, Option<&'a str>) {
        (self.flags() & ARG_QUERY_EXPANSION_MASK, self.separator())
    }

    fn expansion_type(&self) -> QueryExpansion {
        match self.flags() & ARG_QUERY_EXPANSION_MASK {
            ARG_QUERY_EXPANSION_OUTPUT => QueryExpansion::Output,
            ARG_QUERY_EXPANSION_TARGET => QueryExpansion::Target,
            ARG_QUERY_EXPANSION_TARGET_AND_OUTPUT => {
                QueryExpansion::TargetAndOutput(self.separator().map(str::to_owned))
            }
            _ => unreachable!("should be valid: records are only written by the encoder"),
        }
    }

    /// The canonical reprint of the stored expression plus its literal map,
    /// with offsets into the reprint. One walk produces both, so the map's
    /// spans always agree with the returned text.
    pub fn query(&self) -> (String, ResolvedQueryLiterals<ProvidersLabel>) {
        self.expr().query()
    }

    /// The stored expression's literal labels, in expression order (the
    /// same order as the offset-sorted map of the original text).
    pub fn literal_labels(&self, out: &mut Vec<&'a ProvidersLabel>) {
        self.expr().labels(out);
    }

    pub(crate) fn configure(
        &self,
        ctx: &dyn AttrConfigurationContext,
    ) -> QueryMacroBase<ConfiguredProvidersLabel> {
        let mut query = String::new();
        let mut lits = BTreeMap::new();
        self.expr().reprint(&mut query, &mut |span, l| {
            lits.insert(span, ctx.configure_target(l));
        });
        QueryMacroBase {
            expansion_type: self.expansion_type(),
            query: QueryAttrBase {
                query,
                resolved_literals: ResolvedQueryLiterals(lits),
            },
        }
    }
}

impl PartialEq for ArgQueryView<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.flags() == other.flags()
            && self.separator() == other.separator()
            && self.query() == other.query()
    }
}

/// A macro's content as its record stores it: labels and paths by
/// reference, nested text as text, the query expression already encoded.
/// Both the parsed and the decoded form of a macro convert to this, so the
/// record layout and its content hash are defined once.
#[derive(Copy, Clone)]
pub enum MacroParts<'a> {
    Location {
        label: &'a ProvidersLabel,
        dep_kind: MacroDepKind,
    },
    Exe {
        label: &'a ProvidersLabel,
        exec_dep: bool,
    },
    UserUnkeyedPlaceholder(&'a str),
    UserKeyedPlaceholder {
        var: &'a str,
        label: &'a ProvidersLabel,
        arg: Option<&'a str>,
    },
    Source(&'a CoercedPath),
    Query {
        expr: AttrToken,
        expansion: u32,
        sep: Option<&'a str>,
    },
}

/// Writes one macro record; the content hash folds the same components in
/// the same order whichever form the macro came from.
fn write_macro(
    b: &mut CoercedAttrArenaBuilder,
    w: &mut RecordWriter<'_>,
    write_to_file: bool,
    mac: MacroParts<'_>,
) -> buck2_error::Result<()> {
    let (tag, flag, payload, refs, hash_parts): (
        u32,
        u32,
        u32,
        SmallVec<[AttrWord; 2]>,
        SmallVec<[u64; 3]>,
    ) = match mac {
        MacroParts::Location { label, dep_kind } => {
            let (idx, lh) = b.providers_label_idx_and_hash(label);
            (
                ARG_MACRO_TAG_LOCATION,
                dep_kind_bits(dep_kind),
                idx,
                smallvec![],
                smallvec![lh],
            )
        }
        MacroParts::Exe { label, exec_dep } => {
            let (idx, lh) = b.providers_label_idx_and_hash(label);
            (
                ARG_MACRO_TAG_EXE,
                exec_dep as u32,
                idx,
                smallvec![],
                smallvec![lh],
            )
        }
        MacroParts::UserUnkeyedPlaceholder(name) => {
            let AttrToken(sr, sh) = b.encode_str(CoercedAttrKind::String, name)?;
            (ARG_MACRO_TAG_UNKEYED, 0, 0, smallvec![sr], smallvec![sh.0])
        }
        MacroParts::UserKeyedPlaceholder { var, label, arg } => {
            let AttrToken(vr, vh) = b.encode_str(CoercedAttrKind::String, var)?;
            let (idx, lh) = b.providers_label_idx_and_hash(label);
            let mut refs: SmallVec<[AttrWord; 2]> = smallvec![vr];
            let mut parts: SmallVec<[u64; 3]> = smallvec![vh.0, lh];
            let mut flag = 0;
            if let Some(a) = arg {
                let AttrToken(ar, ah) = b.encode_str(CoercedAttrKind::String, a)?;
                refs.push(ar);
                parts.push(ah.0);
                flag = 1;
            }
            (ARG_MACRO_TAG_KEYED, flag, idx, refs, parts)
        }
        MacroParts::Source(path) => {
            let (idx, path_hash) = b.source_path_idx_and_hash(path);
            (
                ARG_MACRO_TAG_SOURCE,
                0,
                idx,
                smallvec![],
                smallvec![path_hash],
            )
        }
        MacroParts::Query {
            expr,
            expansion,
            sep,
        } => {
            let mut refs: SmallVec<[AttrWord; 2]> = smallvec![expr.0];
            let mut parts: SmallVec<[u64; 3]> = smallvec![expr.1.0];
            let mut flags = expansion;
            if let Some(sp) = sep {
                let AttrToken(sr, sh) = b.encode_str(CoercedAttrKind::String, sp)?;
                refs.push(sr);
                parts.push(sh.0);
                flags |= ARG_QUERY_HAS_SEP;
            }
            (ARG_MACRO_TAG_QUERY, flags, 0, refs, parts)
        }
    };
    let wtf_bit = if write_to_file { ARG_MACRO_WTF_BIT } else { 0 };
    w.head(tag | (flag << ARG_MACRO_FLAG_SHIFT) | wtf_bit);
    w.head(payload);
    w.head(refs.len() as u32);
    for r in refs {
        w.r(r);
    }
    w.hash(&(tag, flag, write_to_file, &hash_parts));
    Ok(())
}

impl CoercedAttrArenaBuilder {
    /// Encode an `arg()` value: simple text and macros as records.
    pub fn encode_arg(
        &mut self,
        arg: &StringWithMacros<ProvidersLabel>,
    ) -> buck2_error::Result<AttrToken> {
        match arg {
            StringWithMacros::StringPart(part) => self.encode_arg_simple(part),
            StringWithMacros::ManyParts(parts) => {
                let parts = parts
                    .iter()
                    .map(|p| self.encode_arg_part(p))
                    .collect::<buck2_error::Result<SmallVec<[AttrToken; 8]>>>()?;
                self.encode_arg_parts(&parts)
            }
        }
    }

    /// Encode one arg part: string parts are `ArgSimple` text records;
    /// macro parts dedup as `ArgMacro` records with labels in the label
    /// table.
    fn encode_arg_part(
        &mut self,
        part: &StringWithMacrosPart<ProvidersLabel>,
    ) -> buck2_error::Result<AttrToken> {
        let (write_to_file, m) = match part {
            StringWithMacrosPart::String(s) => {
                return self.encode_str(CoercedAttrKind::ArgSimple, s);
            }
            StringWithMacrosPart::Macro(write_to_file, m) => (*write_to_file, m),
        };
        let parts = match m {
            MacroBase::Location { label, dep_kind } => MacroParts::Location {
                label,
                dep_kind: *dep_kind,
            },
            MacroBase::Exe { label, exec_dep } => MacroParts::Exe {
                label,
                exec_dep: *exec_dep,
            },
            MacroBase::UserUnkeyedPlaceholder(name) => MacroParts::UserUnkeyedPlaceholder(name),
            MacroBase::UserKeyedPlaceholder(box (var, label, arg)) => {
                MacroParts::UserKeyedPlaceholder {
                    var,
                    label,
                    arg: arg.as_deref(),
                }
            }
            MacroBase::Source(path) => MacroParts::Source(path),
            MacroBase::InGroup(..) => {
                return Err(internal_error!(
                    "coerced arg macros are never in an execution group"
                ));
            }
            MacroBase::Query(q) => {
                let parsed = buck2_query_parser::parse_expr(&q.query.query)?;
                let expr =
                    self.encode_query_expr(&parsed, &q.query.query, &q.query.resolved_literals)?;
                let (expansion, sep) = match &q.expansion_type {
                    QueryExpansion::Output => (ARG_QUERY_EXPANSION_OUTPUT, None),
                    QueryExpansion::Target => (ARG_QUERY_EXPANSION_TARGET, None),
                    QueryExpansion::TargetAndOutput(sep) => {
                        (ARG_QUERY_EXPANSION_TARGET_AND_OUTPUT, sep.as_deref())
                    }
                };
                MacroParts::Query {
                    expr,
                    expansion,
                    sep,
                }
            }
        };
        encode_kind::<kinds::ArgMacro>(self, (write_to_file, parts))
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::execution_types::execution::ExecutionPlatformResolution;
    use buck2_core::provider::label::ConfiguredProvidersLabel;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_util::arc_str::ArcStr;

    use super::*;

    fn tool_label() -> ConfiguredProvidersLabel {
        ConfiguredProvidersLabel::default_for(ConfiguredTargetLabel::testing_parse(
            "root//:tool",
            ConfigurationData::testing_new(),
        ))
    }

    fn with_macro(
        macro_: MacroBase<ConfiguredProvidersLabel>,
    ) -> StringWithMacros<ConfiguredProvidersLabel> {
        StringWithMacros::ManyParts(Box::new([
            StringWithMacrosPart::String(ArcStr::from("--tool=")),
            StringWithMacrosPart::Macro(false, macro_),
        ]))
    }

    fn resolution() -> ExecutionPlatformResolution {
        ExecutionPlatformResolution::new_for_testing(None, Vec::new())
    }

    #[test]
    fn test_anon_target_resolution_kept_only_for_tools() {
        let resolution = resolution();
        let flag = ConfiguredStringWithMacros::new(
            StringWithMacros::StringPart(ArcStr::from("--flag")),
            true,
            Some(&resolution),
        );
        assert!(flag.anon_target_compatible());
        assert!(flag.anon_target_tool_resolution().is_none());
        assert!(Arc::ptr_eq(flag.anon_target.as_ref().unwrap(), &NO_TOOLS));

        let target_dep = ConfiguredStringWithMacros::new(
            with_macro(MacroBase::Exe {
                label: tool_label(),
                exec_dep: false,
            }),
            true,
            Some(&resolution),
        );
        assert!(target_dep.anon_target_compatible());
        assert!(target_dep.anon_target_tool_resolution().is_none());

        for tool in [
            MacroBase::Exe {
                label: tool_label(),
                exec_dep: true,
            },
            MacroBase::Location {
                label: tool_label(),
                dep_kind: MacroDepKind::Exec,
            },
            MacroBase::Location {
                label: tool_label(),
                dep_kind: MacroDepKind::Toolchain,
            },
        ] {
            let arg = ConfiguredStringWithMacros::new(with_macro(tool), true, Some(&resolution));
            assert!(arg.anon_target_compatible());
            assert!(arg.anon_target_tool_resolution().is_some(), "{arg}");
        }
    }

    #[test]
    fn test_not_anon_target_compatible_keeps_no_resolution() {
        let resolution = resolution();
        let arg = ConfiguredStringWithMacros::new(
            with_macro(MacroBase::Exe {
                label: tool_label(),
                exec_dep: true,
            }),
            false,
            Some(&resolution),
        );
        assert!(!arg.anon_target_compatible());
        assert!(arg.anon_target_tool_resolution().is_none());
    }

    #[test]
    fn test_anon_target_resolution_not_in_equality() {
        let tool = || {
            with_macro(MacroBase::Exe {
                label: tool_label(),
                exec_dep: true,
            })
        };
        let resolution = resolution();
        assert_eq!(
            ConfiguredStringWithMacros::new(tool(), true, Some(&resolution)),
            ConfiguredStringWithMacros::new(tool(), true, None),
        );
        assert_ne!(
            ConfiguredStringWithMacros::new(tool(), true, None),
            ConfiguredStringWithMacros::new(tool(), false, None),
        );
    }
}
