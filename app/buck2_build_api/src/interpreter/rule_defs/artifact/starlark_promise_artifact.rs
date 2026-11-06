/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::sync::Arc;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use dupe::Dupe;
use dupe::OptionDupedExt;
use starlark::any::ProvidesStaticType;
use starlark::codemap::FileSpan;
use starlark::environment::GlobalsBuilder;
use starlark::values::Demand;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::list::ListType;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::artifact_groups::ArtifactGroup;
use crate::artifact_groups::PromiseArtifactWrapper;
use crate::artifact_groups::promise::PromiseArtifact;
use crate::interpreter::rule_defs::artifact::ArtifactError;
use crate::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use crate::interpreter::rule_defs::artifact::methods::EitherStarlarkInputArtifact;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::ArtifactFingerprint;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactLike;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkInputArtifactLike;
use crate::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifact;
use crate::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::InputProperties;
use crate::interpreter::rule_defs::cmd_args::WriteToFileMacroVisitor;
use crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::command_line_arg_like_impl;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum PromiseArtifactError {
    #[error("cannot access {1} on unresolved promise artifact ({0})")]
    MethodUnsupported(StarlarkPromiseArtifact, &'static str),
    #[error(
        "tried to expand unresolved promise artifact `{0}` to command line args, this should only happen after starlark objects are frozen and promises are resolved"
    )]
    UnresolvedAddedToCommandLine(StarlarkPromiseArtifact),
    #[error("promise artifact ({0}) cannot be projected")]
    CannotProject(StarlarkPromiseArtifact),
    #[error(
        "`short_path` was called on promise artifact ({0}), but no short path was promised. To access the short path on an unresolved promise artifact, `short_path` arg needs to be passed into `artifact_promise()`"
    )]
    NoShortPathPromised(StarlarkPromiseArtifact),
    #[error("The promise artifact ({0}) short_path has no file name")]
    PromisedShortPathHasNoFileName(StarlarkPromiseArtifact),
    #[error("Promise artifacts do not support associated artifacts")]
    CannotAddAssociatedArtifacts,
}

/// An artifact wrapper for a StarlarkPromise that will resolve to an artifact
///
/// The `StarlarkPromiseArtifact` is created with `ctx.actions.artifact_promise()` and will
/// not have been bound until promises are resolved at the end of analysis.
#[derive(
    Debug,
    NoSerialize,
    ProvidesStaticType,
    Trace,
    Allocative,
    Clone,
    Hash,
    Eq,
    PartialEq,
    StarlarkPagable
)]
pub struct StarlarkPromiseArtifact {
    #[starlark_pagable(pagable)]
    pub declaration_location: Option<FileSpan>,
    #[starlark_pagable(pagable)]
    pub artifact: PromiseArtifact,
    #[starlark_pagable(pagable)]
    pub short_path: Option<ForwardRelativePathBuf>,
    pub has_content_based_path: bool,
}

starlark_simple_value!(StarlarkPromiseArtifact);

impl Display for StarlarkPromiseArtifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<promise artifact")?;
        if let Some(location) = &self.declaration_location {
            write!(f, " declared at {location}")?;
        }
        if let Some(short_path) = &self.short_path {
            write!(f, " with short_path `{short_path}`")?;
        }
        if let Some(v) = self.artifact.get() {
            write!(f, " resolved to {v}")?;
        }
        write!(f, ">")?;
        Ok(())
    }
}

impl StarlarkPromiseArtifact {
    pub fn new(
        declaration_location: Option<FileSpan>,
        artifact: PromiseArtifact,
        short_path: Option<ForwardRelativePathBuf>,
        has_content_based_path: bool,
    ) -> Self {
        Self {
            declaration_location,
            artifact,
            short_path,
            has_content_based_path,
        }
    }

    pub fn as_artifact(&self) -> ArtifactGroup {
        match self.artifact.get() {
            Some(artifact) => ArtifactGroup::Artifact(artifact.dupe()),
            None => ArtifactGroup::Promise(Arc::new(PromiseArtifactWrapper::new(
                self.artifact.dupe(),
                self.has_content_based_path,
            ))),
        }
    }

    fn short_path_err(&self) -> buck2_error::Result<&ForwardRelativePath> {
        self.short_path
            .as_deref()
            .ok_or_else(|| PromiseArtifactError::NoShortPathPromised(self.clone()).into())
    }

    fn file_name_err(&self) -> buck2_error::Result<&FileName> {
        self.short_path_err()?.file_name().ok_or_else(|| {
            PromiseArtifactError::PromisedShortPathHasNoFileName(self.clone()).into()
        })
    }
}

impl<'v> StarlarkArtifactLike<'v> for StarlarkPromiseArtifact {
    fn with_filename(
        &self,
        f: &dyn for<'b> Fn(&'b FileName) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>> {
        match self.artifact.get() {
            Some(v) => v.get_path().with_filename(f),
            None => Ok(f(self.file_name_err()?)),
        }
    }

    fn is_source(&'v self) -> buck2_error::Result<bool> {
        Ok(false)
    }

    fn owner(&'v self) -> buck2_error::Result<Option<BaseDeferredKey>> {
        match self.artifact.get() {
            Some(v) => Ok(v.owner().duped()),
            None => Err(PromiseArtifactError::MethodUnsupported(self.clone(), "owner").into()),
        }
    }

    fn with_short_path(
        &self,
        f: &dyn for<'b> Fn(&'b ForwardRelativePath) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>> {
        match self.artifact.get() {
            Some(v) => Ok(v.get_path().with_short_path(f)),
            None => Ok(f(self.short_path_err()?)),
        }
    }

    fn fingerprint<'s>(&'s self) -> ArtifactFingerprint<'s>
    where
        'v: 's,
    {
        let id = self.artifact.id().dupe();
        ArtifactFingerprint::Promise { id }
    }
}

impl<'v> StarlarkInputArtifactLike<'v> for StarlarkPromiseArtifact {
    fn get_bound_artifact(&self) -> buck2_error::Result<Artifact> {
        match self.artifact.get() {
            Some(v) => Ok(v.dupe()),
            None => Err(PromiseArtifactError::MethodUnsupported(
                self.clone(),
                "get_bound_artifact",
            )
            .into()),
        }
    }

    /// Gets any associated artifacts that should be materialized along with the bound artifact
    fn get_associated_artifacts(&self) -> Option<&AssociatedArtifacts> {
        None
    }

    fn as_command_line_like(&self) -> &dyn CommandLineArgLike<'v> {
        self
    }

    fn as_output_error(&self) -> buck2_error::Error {
        ArtifactError::PromiseArtifactAsOutput {
            artifact_repr: self.to_string(),
        }
        .into()
    }

    fn get_artifact_group(&self) -> buck2_error::Result<ArtifactGroup> {
        Ok(self.as_artifact())
    }

    fn as_output(&'v self, _this: Value<'v>) -> buck2_error::Result<StarlarkOutputArtifact<'v>> {
        Err(self.as_output_error())
    }

    fn project(
        &'v self,
        path: &ForwardRelativePath,
        hide_prefix: bool,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>> {
        let _ = (path, hide_prefix);
        Err(PromiseArtifactError::CannotProject(self.clone()).into())
    }

    fn without_associated_artifacts(
        &'v self,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>> {
        Ok(EitherStarlarkInputArtifact::PromiseArtifact(self.clone()))
    }

    fn with_associated_artifacts(
        &'v self,
        artifacts: ValueOf<'v, ListType<StarlarkArtifactView<'v>>>,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>> {
        let _unused = artifacts;
        Err(PromiseArtifactError::CannotAddAssociatedArtifacts.into())
    }
}

impl<'v> CommandLineArgLike<'v> for StarlarkPromiseArtifact {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkPromiseArtifact::starlark_type_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        match self.artifact.get() {
            Some(v) => {
                fmt.push_artifact(v)?;
                Ok(())
            }
            None => Err(PromiseArtifactError::UnresolvedAddedToCommandLine(self.clone()).into()),
        }
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        visitor.visit_input(self.as_artifact(), InputProperties::default(), vec![]);
        Ok(())
    }

    fn contains_arg_attr(&self) -> bool {
        false
    }

    fn visit_write_to_file_macros(
        &self,
        _visitor: &mut dyn WriteToFileMacroVisitor,
        _artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        Ok(())
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for StarlarkPromiseArtifact {
    type Type = StarlarkArtifactView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike>(self);
    }
}

#[starlark_module]
#[starlark_types(
    StarlarkPromiseArtifact as PromiseArtifact
)]
pub(crate) fn register_promise_artifact(globals: &mut GlobalsBuilder) {}
