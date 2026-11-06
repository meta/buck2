/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_artifact::artifact::artifact_type::Artifact;
use dupe::Dupe;
use starlark::typing::Ty;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkInputArtifactLike;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::ValueAsInputArtifactLikeUnpack;
use crate::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum UnpackNonPromiseInputArtifactError {
    #[error("Promise artifacts are not accepted here, got `{0}`")]
    PromiseArtifact(String),
}

/// An `Artifact` that is not a promise artifact. This is stricter than the type, so a promise
/// artifact is a conversion error, not a type mismatch.
#[derive(Debug, Clone, Copy)]
pub enum UnpackNonPromiseInputArtifact<'v> {
    Artifact(ValueTyped<'v, StarlarkArtifact>),
    DeclaredArtifact(ValueTyped<'v, StarlarkDeclaredArtifact<'v>>),
}

impl<'v> StarlarkTypeRepr for UnpackNonPromiseInputArtifact<'v> {
    type Canonical = <ValueAsInputArtifactLikeUnpack<'v> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        ValueAsInputArtifactLikeUnpack::starlark_type_repr()
    }
}

impl<'v> UnpackValue<'v> for UnpackNonPromiseInputArtifact<'v> {
    type Error = starlark::Error;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        match ValueAsInputArtifactLikeUnpack::unpack_value_opt(value) {
            None => Ok(None),
            Some(ValueAsInputArtifactLikeUnpack::Artifact(x)) => {
                Ok(Some(UnpackNonPromiseInputArtifact::Artifact(x)))
            }
            Some(ValueAsInputArtifactLikeUnpack::DeclaredArtifact(x)) => {
                Ok(Some(UnpackNonPromiseInputArtifact::DeclaredArtifact(x)))
            }
            Some(ValueAsInputArtifactLikeUnpack::PromiseArtifact(x)) => Err(
                buck2_error::Error::from(UnpackNonPromiseInputArtifactError::PromiseArtifact(
                    x.to_string(),
                ))
                .into(),
            ),
        }
    }
}

impl<'v> UnpackNonPromiseInputArtifact<'v> {
    pub fn artifact(&self) -> buck2_error::Result<Artifact> {
        match self {
            UnpackNonPromiseInputArtifact::Artifact(x) => Ok(x.artifact.dupe()),
            UnpackNonPromiseInputArtifact::DeclaredArtifact(x) => x.get_bound_artifact(),
        }
    }
}
