/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_path::ArtifactPath;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::BaseArtifactKind;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use dupe::Dupe;
use dupe::OptionDupedExt;
use serde::Serialize;
use serde::Serializer;
use starlark::any::ProvidesStaticType;
use starlark::values::Demand;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::list::ListType;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::artifact_groups::ArtifactGroup;
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

/// A wrapper for an `Artifact` that is guaranteed to be bound, such as outputs
/// from dependencies, or source files.
#[derive(
    Debug,
    Dupe,
    Clone,
    PartialEq,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
pub struct StarlarkArtifact {
    #[starlark_pagable(pagable)]
    pub(crate) artifact: Artifact,
    // A set of ArtifactGroups that should be materialized along with the main artifact
    pub(crate) associated_artifacts: AssociatedArtifacts,
}

starlark_simple_value!(StarlarkArtifact);

impl StarlarkArtifact {
    pub fn new(artifact: Artifact) -> Self {
        StarlarkArtifact {
            artifact,
            associated_artifacts: AssociatedArtifacts::new(),
        }
    }

    pub fn artifact(&self) -> Artifact {
        self.artifact.dupe()
    }

    pub fn get_artifact_path(&self) -> ArtifactPath<'_> {
        self.artifact.get_path()
    }
}

impl Display for StarlarkArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // FIXME(ndmitchell): This display is not the same as the underlying Artifact, but they should probably be the same
        write!(
            f,
            "<{} ",
            if self.artifact.is_source() {
                "source artifact"
            } else {
                "build artifact"
            }
        )?;

        // Historically this has used the full path whereas everything uses the short path.
        self.artifact
            .get_path()
            .with_full_path(|p| write!(f, "{p}"))?;

        if let Some(owner) = self.artifact.owner() {
            write!(f, " bound to {owner}")?;
        }

        write!(f, ">")?;

        Ok(())
    }
}

impl Serialize for StarlarkArtifact {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.collect_str(self)
    }
}

impl<'v> StarlarkArtifactLike<'v> for StarlarkArtifact {
    fn with_filename(
        &self,
        f: &dyn for<'b> Fn(&'b FileName) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>> {
        self.artifact.get_path().with_filename(f)
    }

    fn is_source(&'v self) -> buck2_error::Result<bool> {
        Ok(self.artifact.is_source())
    }

    fn owner(&'v self) -> buck2_error::Result<Option<BaseDeferredKey>> {
        Ok(self.artifact.owner().duped())
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

impl<'v> StarlarkInputArtifactLike<'v> for StarlarkArtifact {
    fn as_output_error(&self) -> buck2_error::Error {
        match self.artifact.as_parts().0 {
            BaseArtifactKind::Source(_) => ArtifactError::SourceArtifactAsOutput {
                repr: self.to_string(),
            }
            .into(),
            BaseArtifactKind::Build(b) => ArtifactError::BoundArtifactAsOutput {
                artifact_repr: self.to_string(),
                existing_owner: b.get_path().owner().owner().dupe(),
            }
            .into(),
        }
    }

    fn get_bound_artifact(&self) -> buck2_error::Result<Artifact> {
        Ok(self.artifact.dupe())
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

    fn as_output(&'v self, _this: Value<'v>) -> buck2_error::Result<StarlarkOutputArtifact<'v>> {
        match self.artifact.as_parts().0 {
            BaseArtifactKind::Source(_) => Err(ArtifactError::SourceArtifactAsOutput {
                repr: self.to_string(),
            }
            .into()),
            BaseArtifactKind::Build(b) => Err(ArtifactError::BoundArtifactAsOutput {
                artifact_repr: self.to_string(),
                existing_owner: b.get_path().owner().owner().dupe(),
            }
            .into()),
        }
    }

    fn project(
        &'v self,
        path: &ForwardRelativePath,
        hide_prefix: bool,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>> {
        Ok(EitherStarlarkInputArtifact::Artifact(StarlarkArtifact {
            artifact: self.artifact.dupe().project(path, hide_prefix),
            associated_artifacts: self.associated_artifacts.dupe(),
        }))
    }

    fn without_associated_artifacts(
        &'v self,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>> {
        Ok(EitherStarlarkInputArtifact::Artifact(StarlarkArtifact {
            artifact: self.artifact.dupe(),
            associated_artifacts: AssociatedArtifacts::new(),
        }))
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

        Ok(EitherStarlarkInputArtifact::Artifact(StarlarkArtifact {
            artifact: self.artifact.dupe(),
            associated_artifacts: self.associated_artifacts.union(artifacts),
        }))
    }
}

impl<'v> CommandLineArgLike<'v> for StarlarkArtifact {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkArtifact::starlark_type_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        fmt.push_artifact(&self.artifact)?;
        Ok(())
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        visitor.visit_input(
            ArtifactGroup::Artifact(self.artifact.dupe()),
            InputProperties::default(),
            vec![],
        );
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

#[starlark_value]
impl<'v> StarlarkValue<'v> for StarlarkArtifact {
    type Type = StarlarkArtifactView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike>(self);
    }
}
