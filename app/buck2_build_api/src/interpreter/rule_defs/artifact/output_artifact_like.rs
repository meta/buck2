/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use starlark::values::UnpackValue;

use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use crate::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifactView;

/// The output of a new action: a path to declare, or an artifact that is not bound yet. The
/// bound representations of either type are accepted by the type and rejected where it is used,
/// so that the error can say that the artifact is bound rather than that it has the wrong type.
#[derive(UnpackValue)]
pub enum OutputArtifactArg<'v> {
    Str(&'v str),
    OutputArtifact(StarlarkOutputArtifactView<'v>),
    Artifact(StarlarkArtifactView<'v>),
}
