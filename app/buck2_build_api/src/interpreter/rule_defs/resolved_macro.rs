/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Provides the starlark values representing resolved attrs.arg() attributes.

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_node::attrs::attr_type::arg::ConfiguredStringWithMacros;
use buck2_util::arc_str::ArcStr;
use buck2_util::size_assert;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_complex_value;
use starlark::values::Demand;
use starlark::values::Freeze;
use starlark::values::FrozenValueTyped;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::artifact_groups::ArtifactGroup;
use crate::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkInputArtifactLike;
use crate::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::InputProperties;
use crate::interpreter::rule_defs::cmd_args::WriteToFileMacroVisitor;
use crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::command_line_arg_like_impl;
use crate::interpreter::rule_defs::cmd_args::value::CommandLineArg;
use crate::interpreter::rule_defs::provider::builtin::default_info::DefaultInfo;
use crate::interpreter::rule_defs::resolve_query_macro::ResolvedQueryMacro;

// TODO(cjhopman): Consider making DefaultOutputs implement CommandLineArgLike
// itself, and then a resolved macro is just a CommandLineArgLike.

#[derive(Debug, PartialEq, Trace, Freeze, Allocative, StarlarkPagable)]
pub enum ResolvedMacro<'v> {
    // `default_outputs()` hands out `StarlarkArtifact`s, which requires the frozen form.
    Location(FrozenValueTyped<'v, DefaultInfo<'v>>),
    Source(
        #[trace(static)]
        #[freeze(identity)]
        #[starlark_pagable(pagable)]
        Artifact,
    ),
    /// Holds an arg-like value
    ArgLike(CommandLineArg<'v>),
    /// Holds a resolved query placeholder
    Query(
        #[trace(static)]
        #[freeze(identity)]
        ResolvedQueryMacro,
    ),
}

size_assert::words_of_type!(ResolvedMacro, 2);

impl<'v> Display for ResolvedMacro<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolvedMacro::Location(r) => match r.default_outputs() {
                Ok(outputs) if !outputs.is_empty() => write!(f, "$(location {})", outputs[0]),
                _ => write!(f, "$(location ...)"),
            },
            ResolvedMacro::Source(a) => write!(f, "$(source {a})"),
            ResolvedMacro::ArgLike(x) => Display::fmt(x, f),
            ResolvedMacro::Query(x) => Display::fmt(x, f),
        }
    }
}

pub fn add_output_to_arg(
    fmt: &mut CommandLineBuilder,
    artifact: &StarlarkArtifact,
) -> buck2_error::Result<()> {
    fmt.push_artifact(&artifact.get_bound_artifact()?)?;
    Ok(())
}

fn add_outputs_to_arg(
    fmt: &mut CommandLineBuilder,
    outputs_list: &[StarlarkArtifact],
) -> buck2_error::Result<()> {
    for (i, value) in outputs_list.iter().enumerate() {
        if i != 0 {
            fmt.push_str(" ");
        }
        add_output_to_arg(fmt, value)?;
    }
    Ok(())
}

impl<'v> ResolvedMacro<'v> {
    pub fn add_to_arg(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        match self {
            Self::Source(artifact) => {
                fmt.push_artifact(artifact)?;
            }
            Self::Location(info) => {
                let outputs = info.default_outputs()?;

                add_outputs_to_arg(fmt, &outputs)?;
            }
            Self::ArgLike(command_line_like) => {
                fmt.push_scope_delimiter(" ");
                command_line_like
                    .as_command_line_arg()
                    .add_to_command_line(fmt)?;
                fmt.pop_scope();
            }
            Self::Query(value) => value.add_to_arg(fmt)?,
        };

        Ok(())
    }

    /// Visit the artifacts referenced by this macro.
    pub fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        match self {
            Self::Location(info) => {
                if visitor.inputs_for_path_resolution() {
                    info.for_each_default_output_artifact_only(&mut |artifact| {
                        visitor.visit_input(
                            ArtifactGroup::Artifact(artifact),
                            InputProperties::default(),
                            vec![],
                        )
                    })?;
                } else {
                    info.for_each_output(&mut |i| {
                        visitor.visit_input(i, InputProperties::default(), vec![])
                    })?;
                }
            }
            Self::ArgLike(command_line_like) => {
                command_line_like
                    .as_command_line_arg()
                    .visit_artifacts(visitor)?;
            }
            Self::Query(value) => value.visit_artifacts(visitor)?,
            Self::Source(artifact) => visitor.visit_input(
                ArtifactGroup::Artifact(artifact.dupe()),
                InputProperties::default(),
                vec![],
            ),
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Trace, Freeze, Allocative, StarlarkPagable)]
pub enum ResolvedStringWithMacrosPart<'v> {
    String(
        #[trace(static)]
        #[freeze(identity)]
        #[starlark_pagable(pagable)]
        ArcStr,
    ),
    Macro(/* write_to_file */ bool, ResolvedMacro<'v>),
}

impl<'v> Display for ResolvedStringWithMacrosPart<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(x) => f.write_str(x),
            Self::Macro(b, x) => {
                if *b {
                    write!(f, "@")?;
                }
                Display::fmt(x, f)
            }
        }
    }
}

#[derive(
    Debug,
    PartialEq,
    Trace,
    Freeze,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
pub struct ResolvedStringWithMacros<'v> {
    parts: MiniBoxSlice<ResolvedStringWithMacrosPart<'v>>,
    #[trace(static)]
    #[freeze(identity)]
    #[starlark_pagable(pagable)]
    configured_macros: Option<ConfiguredStringWithMacros>,
}

starlark_complex_value!(pub ResolvedStringWithMacros);

impl<'v> Display for ResolvedStringWithMacros<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"")?;
        for x in &self.parts {
            Display::fmt(x, f)?;
        }
        write!(f, "\"")
    }
}

impl<'v> ResolvedStringWithMacros<'v> {
    pub fn new(
        parts: Vec<ResolvedStringWithMacrosPart<'v>>,
        configured_macros: Option<&ConfiguredStringWithMacros>,
    ) -> Self {
        Self {
            parts: parts.into(),
            configured_macros: configured_macros.cloned(),
        }
    }

    /// Access the `&str` in this ResolvedStringWithMacros, *if* this ResolvedStringWithMacros is
    /// secretely just one String.
    pub fn downcast_str(&self) -> Option<&str> {
        let mut iter = self.parts.iter();
        match (iter.next(), iter.next()) {
            (Some(ResolvedStringWithMacrosPart::String(s)), None) => Some(s),
            _ => None,
        }
    }

    pub fn configured_macros(&self) -> &Option<ConfiguredStringWithMacros> {
        &self.configured_macros
    }
}

impl<'v> CommandLineArgLike<'v> for ResolvedStringWithMacros<'v> {
    fn register_me(&self) {
        command_line_arg_like_impl!(ResolvedStringWithMacros::starlark_type_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        fmt.push_scope_delimiter("");
        for part in &*self.parts {
            match part {
                ResolvedStringWithMacrosPart::String(s) => {
                    fmt.push_str(s);
                }
                ResolvedStringWithMacrosPart::Macro(write_to_file, val) => {
                    if *write_to_file {
                        fmt.push_str("@");
                        fmt.push_next_write_to_file_macro_path()?;
                    } else {
                        val.add_to_arg(fmt)?;
                    }
                }
            }
        }
        fmt.pop_scope();
        Ok(())
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        for part in &*self.parts {
            if let ResolvedStringWithMacrosPart::Macro(write_to_file, val) = part {
                if !*write_to_file || !visitor.inputs_for_path_resolution() {
                    val.visit_artifacts(visitor)?;
                }
            }
        }

        Ok(())
    }

    fn contains_arg_attr(&self) -> bool {
        true
    }

    fn visit_write_to_file_macros(
        &self,
        visitor: &mut dyn WriteToFileMacroVisitor,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        for part in &*self.parts {
            match part {
                ResolvedStringWithMacrosPart::String(_) => {
                    // nop
                }
                ResolvedStringWithMacrosPart::Macro(write_to_file, val) => {
                    if *write_to_file {
                        visitor.visit_write_to_file_macro(val, artifact_path_mapping)?;
                    } else {
                        // nop
                    }
                }
            }
        }
        Ok(())
    }
}

starlark::methods_static!(
    RESOLVED_STRING_WITH_MACROS_METHODS = resolved_string_with_macros_methods
);

#[starlark_value(type = "ResolvedStringWithMacros")]
impl<'v> StarlarkValue<'v> for ResolvedStringWithMacros<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(RESOLVED_STRING_WITH_MACROS_METHODS.methods())
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        if let Some(other) = ResolvedStringWithMacros::from_value(other) {
            Ok(*self == *other)
        } else if let Some(s) = other.unpack_str() {
            Ok(self.downcast_str() == Some(s))
        } else {
            Ok(false)
        }
    }

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike>(self);
    }
}

#[starlark_module]
fn resolved_string_with_macros_methods(builder: &mut MethodsBuilder) {
    fn startswith<'v>(
        this: &ResolvedStringWithMacros<'v>,
        #[starlark(require = pos)] prefix: &str,
    ) -> starlark::Result<bool> {
        match this.parts.first() {
            Some(ResolvedStringWithMacrosPart::String(s)) => Ok(s.starts_with(prefix)),
            _ => Ok(false),
        }
    }
}

#[starlark_module]
#[starlark_types(
    ResolvedStringWithMacros<'static> as ResolvedStringWithMacros
)]
pub(crate) fn register_string_with_macros(globals: &mut GlobalsBuilder) {}

#[cfg(test)]
mod tests {

    use starlark::values::Heap;
    use starlark::values::StarlarkType;

    use super::*;

    fn make_string_resolved<'v>(s: &str) -> ResolvedStringWithMacros<'v> {
        ResolvedStringWithMacros::new(
            vec![ResolvedStringWithMacrosPart::String(ArcStr::from(s))],
            None,
        )
    }

    #[test]
    fn test_equals_matching_string() {
        Heap::temp(|heap| {
            let resolved = make_string_resolved("-matching-flag");
            let str_val = heap.alloc_str("-matching-flag").to_value();
            assert_eq!(resolved.equals(str_val).unwrap(), true);
        });
    }

    #[test]
    fn test_equals_non_matching_string() {
        Heap::temp(|heap| {
            let resolved = make_string_resolved("-resolved-flag");
            let str_val = heap.alloc_str("-str-flag").to_value();
            assert_eq!(resolved.equals(str_val).unwrap(), false);
        });
    }

    #[test]
    fn test_equals_resolved_string_vs_resolved_string() {
        Heap::temp(|heap| {
            let resolved = make_string_resolved("-resolved-flag");
            let other_val = heap.alloc(make_string_resolved("-resolved-flag"));
            assert_eq!(resolved.equals(other_val).unwrap(), true);
        });
    }

    #[test]
    fn test_equals_resolved_string_vs_non_string_value() {
        Heap::temp(|heap| {
            let resolved = make_string_resolved("-Wno-error");
            let int_val = heap.alloc(42);
            assert_eq!(resolved.equals(int_val).unwrap(), false);
        });
    }

    #[test]
    fn test_equals_resolved_macro_vs_non_string_value() {
        Heap::temp(|heap| {
            let resolved = ResolvedStringWithMacros::new(
                vec![ResolvedStringWithMacrosPart::Macro(
                    false,
                    ResolvedMacro::Query(ResolvedQueryMacro::Targets(Default::default())),
                )],
                None,
            );
            let int_val = heap.alloc(42);
            assert_eq!(resolved.equals(int_val).unwrap(), false);
        });
    }
}
