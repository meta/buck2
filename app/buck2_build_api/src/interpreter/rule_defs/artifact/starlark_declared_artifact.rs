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

use allocative::Allocative;
use buck2_artifact::artifact::artifact_path::ArtifactPath;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::DeclaredArtifact;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_error::BuckErrorContext;
use buck2_error::buck2_error;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::codemap::FileSpan;
use starlark::values::AllocValue;
use starlark::values::Demand;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::ListType;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::artifact_groups::ArtifactGroup;
use crate::interpreter::rule_defs::artifact::ArtifactError;
use crate::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use crate::interpreter::rule_defs::artifact::methods::EitherStarlarkInputArtifact;
use crate::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
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

/// A wrapper for an artifact that has been declared in the user's implementation function.
///
/// The `DeclaredArtifact` may or may not have been bound yet, but must be by the end of the
/// user's implementation function. This turns into a `StarlarkArtifact` when frozen, and
/// /must/ have had `ensure_bound()` called on it successfully before freezing.
#[derive(Clone, Debug, Dupe, NoSerialize, ProvidesStaticType, Trace, Allocative)]
pub struct StarlarkDeclaredArtifact<'v> {
    // FileSpan is not Hash, and the location is not really relevant for equality.
    // We also expect each artifact to be declared at one unique location anyway.
    pub(super) declaration_location: Option<FileSpan>,
    pub(super) artifact: DeclaredArtifact<'v>,
    // A set of ArtifactGroups that should be materialized along with the main artifact
    pub(super) associated_artifacts: AssociatedArtifacts,
}

impl Display for StarlarkDeclaredArtifact<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<build artifact {}", self.artifact)?;
        if let Some(location) = &self.declaration_location {
            write!(f, " declared at {location}")?;
        }
        if let Some(label) = self.artifact.owner() {
            write!(f, " bound to {label}")?;
        }
        write!(f, ">")
    }
}

impl<'v> StarlarkDeclaredArtifact<'v> {
    pub fn new(
        declaration_location: Option<FileSpan>,
        artifact: DeclaredArtifact<'v>,
        associated_artifacts: AssociatedArtifacts,
    ) -> Self {
        StarlarkDeclaredArtifact {
            declaration_location,
            artifact,
            associated_artifacts,
        }
    }

    pub fn output_artifact(&self) -> OutputArtifact<'v> {
        self.artifact.as_output()
    }

    pub fn get_artifact_path(&self) -> ArtifactPath<'_> {
        self.artifact.get_path()
    }

    pub fn with_extended_associated_artifacts(
        &self,
        extra_associated_artifacts: AssociatedArtifacts,
    ) -> Self {
        let merged = self.associated_artifacts.union(extra_associated_artifacts);
        Self {
            declaration_location: self.declaration_location.clone(),
            artifact: self.artifact.dupe(),
            associated_artifacts: merged,
        }
    }

    pub fn has_content_based_path(&self) -> bool {
        self.artifact.has_content_based_path()
    }
}

impl<'v> StarlarkArtifactLike<'v> for StarlarkDeclaredArtifact<'v> {
    fn with_filename(
        &self,
        f: &dyn for<'b> Fn(&'b FileName) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>> {
        self.artifact.get_path().with_filename(f)
    }

    fn is_source(&'v self) -> buck2_error::Result<bool> {
        Ok(false)
    }

    fn owner(&'v self) -> buck2_error::Result<Option<BaseDeferredKey>> {
        Ok(self.artifact.owner())
    }

    fn with_short_path(
        &self,
        f: &dyn for<'b> Fn(&'b ForwardRelativePath) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>> {
        Ok(self.artifact.get_path().with_short_path(f))
    }

    fn fingerprint<'s>(&'s self) -> ArtifactFingerprint<'s>
    where
        'v: 's,
    {
        let path = self.artifact.get_path();
        let associated_artifacts = self.get_associated_artifacts();
        ArtifactFingerprint::Normal {
            path,
            associated_artifacts,
            is_output: false,
        }
    }
}

impl<'v> StarlarkInputArtifactLike<'v> for StarlarkDeclaredArtifact<'v> {
    fn as_output_error(&self) -> buck2_error::Error {
        // This shouldn't ever be called for StarlarkDeclaredArtifact
        buck2_error!(
            buck2_error::ErrorTag::Tier0,
            "error trying to use declared artifact as an output, this indicates an internal buck error"
        )
    }

    fn get_bound_artifact(&self) -> buck2_error::Result<Artifact> {
        Ok(self.artifact.dupe().ensure_bound()?.into_artifact())
    }

    fn get_associated_artifacts(&self) -> Option<&AssociatedArtifacts> {
        Some(&self.associated_artifacts)
    }

    fn as_command_line_like(&self) -> &dyn CommandLineArgLike<'v> {
        self
    }

    fn get_artifact_group(&self) -> buck2_error::Result<ArtifactGroup> {
        Ok(ArtifactGroup::Artifact(self.get_bound_artifact()?))
    }

    fn as_output(&'v self, this: Value<'v>) -> buck2_error::Result<StarlarkOutputArtifact<'v>> {
        Ok(StarlarkOutputArtifact::new(
            ValueTyped::<StarlarkDeclaredArtifact>::new_err(this)
                .internal_error("Type must have been checked earlier")?,
        ))
    }

    fn project(
        &'v self,
        path: &ForwardRelativePath,
        hide_prefix: bool,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>> {
        // Not sure if this.declaration_location is or the project() call is more appropriate here.
        Ok(EitherStarlarkInputArtifact::DeclaredArtifact(
            StarlarkDeclaredArtifact {
                declaration_location: self.declaration_location.dupe(),
                artifact: self.artifact.project(path, hide_prefix),
                associated_artifacts: self.associated_artifacts.dupe(),
            },
        ))
    }

    fn without_associated_artifacts(
        &'v self,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>> {
        Ok(EitherStarlarkInputArtifact::DeclaredArtifact(
            StarlarkDeclaredArtifact {
                declaration_location: self.declaration_location.dupe(),
                artifact: self.artifact.dupe(),
                associated_artifacts: AssociatedArtifacts::new(),
            },
        ))
    }

    fn with_associated_artifacts(
        &'v self,
        artifacts: ValueOf<'v, ListType<StarlarkArtifactView<'v>>>,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>> {
        let artifacts = artifacts
            .try_items()
            .map(|a| a?.as_input_artifact_like().get_artifact_group())
            .collect::<buck2_error::Result<Vec<_>>>()?;

        let artifacts = AssociatedArtifacts::from(artifacts);

        Ok(EitherStarlarkInputArtifact::DeclaredArtifact(
            StarlarkDeclaredArtifact {
                declaration_location: self.declaration_location.dupe(),
                artifact: self.artifact.dupe(),
                associated_artifacts: self.associated_artifacts.union(artifacts),
            },
        ))
    }
}

impl<'v> CommandLineArgLike<'v> for StarlarkDeclaredArtifact<'v> {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkDeclaredArtifact::starlark_type_repr());
    }

    fn add_to_command_line(
        &self,
        _fmt: &mut CommandLineBuilder<'v, '_>,
    ) -> buck2_error::Result<()> {
        // TODO: proper error message
        Err(buck2_error!(
            buck2_error::ErrorTag::Tier0,
            "proper error here; we should not be adding mutable starlark objects to clis"
        ))
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        visitor.visit_declared_artifact(
            self.artifact.dupe(),
            InputProperties::default(),
            vec![],
        )?;

        if !visitor.inputs_for_path_resolution() {
            self.associated_artifacts
                .iter()
                .for_each(|ag| visitor.visit_input(ag.dupe(), InputProperties::default(), vec![]));
        }
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

impl<'v> Freeze<'v> for StarlarkDeclaredArtifact<'v> {
    type Frozen<'fv> = StarlarkArtifact;

    fn freeze<'fv>(self, _freezer: &Freezer<'v, 'fv>) -> FreezeResult<StarlarkArtifact> {
        // ensure_bound() moves out of self and so we can't construct the error
        // after calling that, so we need to check first.
        if !self.artifact.is_bound() {
            return Err(FreezeError::new(
                ArtifactError::DeclaredArtifactWasNotBound {
                    repr: self.to_string(),
                }
                .to_string(),
            ));
        }
        // TODO(minglunli): Check to see if the error carries a context
        let artifact = match self.artifact.ensure_bound() {
            Ok(artifact) => artifact,
            Err(e) => return Err(FreezeError::new(e.to_string())),
        }
        .into_artifact();
        Ok(StarlarkArtifact {
            artifact,
            associated_artifacts: self.associated_artifacts,
        })
    }
}

impl<'v> AllocValue<'v> for StarlarkDeclaredArtifact<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for StarlarkDeclaredArtifact<'v> {
    type Type = StarlarkArtifactView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike>(self);
    }
}
