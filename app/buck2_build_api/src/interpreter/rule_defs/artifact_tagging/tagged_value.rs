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
use starlark::values::Freeze;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::starlark_value;

use super::ArtifactTag;
use super::TaggedVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;

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
#[display("TaggedValue({}, tagged {})", inner, tag)]
pub struct StarlarkTaggedValue<'v> {
    inner: Value<'v>,
    #[freeze(identity)]
    #[starlark_pagable(pagable)]
    tag: ArtifactTag,
    inputs_only: bool,
}

impl<'v> StarlarkTaggedValue<'v> {
    pub fn new(inner: Value<'v>, tag: ArtifactTag) -> Self {
        Self {
            inner,
            tag,
            inputs_only: false,
        }
    }

    pub fn inputs_only(inner: Value<'v>, tag: ArtifactTag) -> Self {
        Self {
            inner,
            tag,
            inputs_only: true,
        }
    }
}

starlark_complex_value!(pub StarlarkTaggedValue);

starlark::methods_static!(TAGGED_VALUE_METHODS = tagged_value_methods);

#[starlark_value(type = "TaggedValue")]
impl<'v> StarlarkValue<'v> for StarlarkTaggedValue<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(TAGGED_VALUE_METHODS.methods())
    }
}

/// Opaque type returned by [`ArtifactTag.tag_artifacts()`](../ArtifactTag#artifacttagtag_artifacts)
/// or [`ArtifactTag.tag_inputs()`](../ArtifactTag#artifacttagtag_inputs) for non-command-line like values.
///
/// For complete documentation, see [`ctx.actions.artifact_tag()`](../AnalysisActions#analysisactionsartifact_tag).
#[starlark_module]
fn tagged_value_methods(_: &mut MethodsBuilder) {}

impl<'v> StarlarkTaggedValue<'v> {
    pub fn value(&self) -> Value<'v> {
        self.inner
    }

    pub fn wrap_visitor<'a, 'b>(
        &'a self,
        visitor: &'b mut dyn CommandLineArtifactVisitor<'v>,
    ) -> TaggedVisitor<'a, 'b, 'v> {
        TaggedVisitor::wrap(&self.tag, self.inputs_only, visitor)
    }
}

#[starlark_module]
#[starlark_types(StarlarkTaggedValue<'_> as TaggedValue)]
pub(crate) fn register_tagged_value(globals: &mut GlobalsBuilder) {}
