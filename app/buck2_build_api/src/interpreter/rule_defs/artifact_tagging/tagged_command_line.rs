/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use derive_more::Display;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_complex_value;
use starlark::starlark_module;
use starlark::values::Demand;
use starlark::values::Freeze;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::starlark_value;

use super::StarlarkTaggedValue;
use crate::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::WriteToFileMacroVisitor;
use crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::command_line_arg_like_impl;
use crate::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;

#[derive(
    Debug,
    Clone,
    Trace,
    Freeze,
    Display,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[derive(NoSerialize)] // TODO make artifacts serializable
#[display("StarlarkTaggedCommandLine({})", inner)]
pub struct StarlarkTaggedCommandLine<'v> {
    inner: StarlarkTaggedValue<'v>,
}

impl<'v> StarlarkTaggedCommandLine<'v> {
    pub fn new(inner: StarlarkTaggedValue<'v>) -> Self {
        Self { inner }
    }
}

starlark_complex_value!(pub StarlarkTaggedCommandLine);

starlark::methods_static!(TAGGED_COMMAND_LINE_METHODS = tagged_command_line_methods);

#[starlark_value(type = "TaggedCommandLine")]
impl<'v> StarlarkValue<'v> for StarlarkTaggedCommandLine<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(TAGGED_COMMAND_LINE_METHODS.methods())
    }

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike>(self);
    }
}

/// Internal wrapper type returned by [`ArtifactTag.tag_artifacts()`](../ArtifactTag#artifacttagtag_artifacts)
/// or [`ArtifactTag.tag_inputs()`](../ArtifactTag#artifacttagtag_inputs) for command-line values.
///
/// For complete documentation, see [`ctx.actions.artifact_tag()`](../AnalysisActions#analysisactionsartifact_tag).
#[starlark_module]
fn tagged_command_line_methods(_: &mut MethodsBuilder) {}

impl<'v> CommandLineArgLike<'v> for StarlarkTaggedCommandLine<'v> {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkTaggedCommandLine::get_type_starlark_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        ValueAsCommandLineLike::unpack_value_err(self.inner.value())?
            .0
            .add_to_command_line(fmt)
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        let mut visitor = self.inner.wrap_visitor(visitor);

        ValueAsCommandLineLike::unpack_value_err(self.inner.value())?
            .0
            .visit_artifacts(&mut visitor)
    }

    fn contains_arg_attr(&self) -> bool {
        ValueAsCommandLineLike::unpack(self.inner.value())
            .is_some_and(|inner| inner.0.contains_arg_attr())
    }

    fn visit_write_to_file_macros(
        &self,
        visitor: &mut dyn WriteToFileMacroVisitor,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        ValueAsCommandLineLike::unpack_value_err(self.inner.value())?
            .0
            .visit_write_to_file_macros(visitor, artifact_path_mapping)
    }
}

#[starlark_module]
#[starlark_types(
    StarlarkTaggedCommandLine<'_> as TaggedCommandLine
)]
pub(crate) fn register_tagged_command_line(globals: &mut GlobalsBuilder) {}
