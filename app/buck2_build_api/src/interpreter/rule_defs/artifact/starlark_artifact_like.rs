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
use std::hash::Hash;
use std::hash::Hasher;

use buck2_artifact::artifact::artifact_path::ArtifactPath;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use starlark::collections::StarlarkHasher;
use starlark::environment::Methods;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StringValue;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::ListType;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::artifact_groups::ArtifactGroup;
use crate::artifact_groups::promise::PromiseArtifactId;
use crate::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use crate::interpreter::rule_defs::artifact::methods::EitherStarlarkInputArtifact;
use crate::interpreter::rule_defs::artifact::methods::artifact_methods;
use crate::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use crate::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use crate::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifact;
use crate::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifactView;
use crate::interpreter::rule_defs::artifact::starlark_promise_artifact::StarlarkPromiseArtifact;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;

pub trait StarlarkArtifactLike<'v>: Display {
    fn with_filename(
        &self,
        f: &dyn for<'b> Fn(&'b FileName) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>>;

    fn is_source(&'v self) -> buck2_error::Result<bool>;

    fn owner(&'v self) -> buck2_error::Result<Option<BaseDeferredKey>>;

    fn with_short_path(
        &self,
        f: &dyn for<'b> Fn(&'b ForwardRelativePath) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>>;

    /// It's very important that the Hash/Eq of the StarlarkArtifactLike things doesn't change
    /// during freezing, otherwise Starlark invariants are broken. Use the fingerprint
    /// as the inputs to Hash/Eq to ensure they are consistent
    fn fingerprint<'s>(&'s self) -> ArtifactFingerprint<'s>
    where
        'v: 's;

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        Ok(ArtifactOrOutputArtifact::unpack_value_opt(other)
            .is_some_and(|other| self.fingerprint() == other.as_artifact_like().fingerprint()))
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        self.fingerprint().hash(hasher);
        Ok(())
    }
}

/// A trait representing starlark representations of input artifacts.
///
/// Not implemented for `OutputArtifact`
pub trait StarlarkInputArtifactLike<'v>: StarlarkArtifactLike<'v> {
    /// Returns an apppropriate error for when this is used in a location that expects an output declaration.
    fn as_output_error(&self) -> buck2_error::Error;

    /// Gets the bound main artifact, or errors if the artifact is not bound
    fn get_bound_artifact(&self) -> buck2_error::Result<Artifact>;

    /// Gets any associated artifacts that should be materialized along with the bound artifact
    fn get_associated_artifacts(&self) -> Option<&AssociatedArtifacts>;

    /// Return an interface for frozen and bound artifacts (`StarlarkArtifact`) to add to a CLI
    ///
    /// Returns None if this artifact isn't the correct type to be added to a CLI object
    fn as_command_line_like(&self) -> &dyn CommandLineArgLike<'v>;

    /// Gets a copy of the StarlarkArtifact, ensuring that the artifact is bound.
    fn get_bound_starlark_artifact(&self) -> buck2_error::Result<StarlarkArtifact> {
        let artifact = self.get_bound_artifact()?;
        let associated_artifacts = self.get_associated_artifacts();
        Ok(StarlarkArtifact {
            artifact,
            associated_artifacts: associated_artifacts
                .map_or(AssociatedArtifacts::new(), |a| a.clone()),
        })
    }

    /// Gets the artifact group.
    fn get_artifact_group(&self) -> buck2_error::Result<ArtifactGroup>;

    fn as_output(&'v self, this: Value<'v>) -> buck2_error::Result<StarlarkOutputArtifact<'v>>;

    fn project(
        &'v self,
        path: &ForwardRelativePath,
        hide_prefix: bool,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>>;

    fn without_associated_artifacts(
        &'v self,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>>;

    fn with_associated_artifacts(
        &'v self,
        artifacts: ValueOf<'v, ListType<StarlarkArtifactView<'v>>>,
    ) -> buck2_error::Result<EitherStarlarkInputArtifact<'v>>;
}

/// An `Artifact` in any of its representations: the receiver of the type's methods, and a
/// binding for a native parameter of type `Artifact`.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub enum StarlarkArtifactView<'v> {
    Artifact(ValueTyped<'v, StarlarkArtifact>),
    DeclaredArtifact(ValueTyped<'v, StarlarkDeclaredArtifact<'v>>),
    PromiseArtifact(ValueTyped<'v, StarlarkPromiseArtifact>),
}

impl<'v> StarlarkArtifactView<'v> {
    pub fn as_input_artifact_like(self) -> &'v dyn StarlarkInputArtifactLike<'v> {
        match self {
            StarlarkArtifactView::Artifact(x) => x.as_ref(),
            StarlarkArtifactView::DeclaredArtifact(x) => x.as_ref(),
            StarlarkArtifactView::PromiseArtifact(x) => x.as_ref(),
        }
    }
}

starlark::methods_static!(STARLARK_ARTIFACT_METHODS = artifact_methods);

#[starlark_value(type = "Artifact")]
impl<'v> StarlarkType<'v> for StarlarkArtifactView<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(STARLARK_ARTIFACT_METHODS.methods())
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        StarlarkArtifactLike::equals(self.as_input_artifact_like(), other)
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        StarlarkArtifactLike::write_hash(self.as_input_artifact_like(), hasher)
    }
}

/// An `OutputArtifact` or an `Artifact`: the receiver of the methods the two types share.
#[derive(StarlarkTypeRepr, UnpackValue)]
pub enum ArtifactOrOutputArtifact<'v> {
    OutputArtifact(StarlarkOutputArtifactView<'v>),
    Artifact(StarlarkArtifactView<'v>),
}

impl<'v> ArtifactOrOutputArtifact<'v> {
    pub fn as_artifact_like(self) -> &'v dyn StarlarkArtifactLike<'v> {
        match self {
            ArtifactOrOutputArtifact::OutputArtifact(x) => x.as_artifact_like(),
            ArtifactOrOutputArtifact::Artifact(x) => x.as_input_artifact_like(),
        }
    }
}

#[derive(PartialEq)]
pub enum ArtifactFingerprint<'a> {
    Normal {
        path: ArtifactPath<'a>,
        associated_artifacts: Option<&'a AssociatedArtifacts>,
        is_output: bool,
    },
    Promise {
        id: PromiseArtifactId,
    },
}

impl Hash for ArtifactFingerprint<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match &self {
            ArtifactFingerprint::Normal {
                path,
                associated_artifacts,
                is_output,
            } => {
                path.hash(state);
                is_output.hash(state);
                if let Some(associated) = associated_artifacts {
                    associated.len().hash(state);
                    associated.iter().for_each(|ag| ag.hash(state));
                }
            }
            ArtifactFingerprint::Promise { id } => id.hash(state),
        }
    }
}
