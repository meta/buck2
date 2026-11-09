/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub(crate) mod artifact_type;
pub mod associated;
pub(crate) mod methods;
pub mod output_artifact_like;
pub mod starlark_artifact;
pub mod starlark_artifact_like;
pub mod starlark_artifact_value;
pub mod starlark_declared_artifact;
pub mod starlark_output_artifact;
pub mod starlark_promise_artifact;

use std::fmt::Debug;

use buck2_core::deferred::base_deferred_key::BaseDeferredKey;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub(crate) enum ArtifactError {
    #[error("expected artifact {repr} to be used as the output of an action, but it was not")]
    DeclaredArtifactWasNotBound { repr: String },
    #[error(
        "attempted to use source artifact {repr} as the output of an action. Source \
        artifacts may not be outputs."
    )]
    SourceArtifactAsOutput { repr: String },
    #[error(
        "attempted to use artifact {artifact_repr} as the output of an action, but \
        it was already used by another action in {existing_owner}"
    )]
    BoundArtifactAsOutput {
        artifact_repr: String,
        existing_owner: BaseDeferredKey,
    },
    #[error(
        "attempted to use promise artifact {artifact_repr} as the output of an action, but \
        only declared artifacts can be used as an output"
    )]
    PromiseArtifactAsOutput { artifact_repr: String },
}
