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
use starlark::values::ValueTyped;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use crate::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use crate::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifact;

#[derive(StarlarkTypeRepr, UnpackValue)]
pub enum OutputArtifactArg<'v> {
    Str(&'v str),
    OutputArtifact(ValueTyped<'v, StarlarkOutputArtifact<'v>>),
    DeclaredArtifact(ValueTyped<'v, StarlarkDeclaredArtifact<'v>>),
    /// This for error reporting.
    WrongArtifact(StarlarkArtifactView<'v>),
}
