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
use std::fmt::Formatter;

use allocative::Allocative;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::starlark_value;

use crate::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkInputArtifactLike;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum ValidationSpecError {
    #[error("Name of validation spec should not be empty")]
    EmptyName,
    #[error("Validation result artifact should be a build artifact, not a source one.")]
    ValidationResultIsSourceArtifact,
}

/// A single, identifiable validation attached to a target.
///
/// A `ValidationSpec` pairs a stable name with a build artifact that, once
/// produced, is parsed by Buck2 to decide pass/fail. Group one or more
/// specs into a `ValidationInfo` provider to attach them to a target.
///
/// The `validation_result` artifact must be a build artifact (declared via
/// `ctx.actions.declare_output(...)` and bound to an action) — source
/// artifacts are rejected because validations are expected to be derived,
/// reproducible outputs.
///
/// See the [Validations guide](https://buck2.build/docs/rule_authors/validation/)
/// for the end-to-end story.
#[derive(
    Debug,
    Trace,
    NoSerialize,
    ProvidesStaticType,
    Allocative,
    Freeze,
    StarlarkPagable
)]
#[freeze(validator = validate_validation_spec)]
pub struct StarlarkValidationSpec<'v> {
    /// Name identifying this validation. Must be non-empty and unique within
    /// the enclosing `ValidationInfo`. Surfaces in CLI output and is the
    /// handle used by `--enable-optional-validations <name>`.
    name: StringValue<'v>,
    /// Build artifact produced by the validator. After the action that
    /// produces it runs, Buck2 reads the file as UTF-8 JSON and expects
    /// the following shape:
    ///
    /// ```json
    /// {
    ///   "version": 1,
    ///   "data": {
    ///     "status": "success",
    ///     "message": "optional human-readable detail"
    ///   }
    /// }
    /// ```
    ///
    /// - `version` (int, required): schema version. Currently `1`.
    /// - `data.status` (string, required): `"success"` or `"failure"`.
    /// - `data.message` (string, optional): shown to the user; supply on
    ///   failure so the diagnostic is actionable.
    ///
    /// Buck2 surfaces three distinct errors if the file does not conform:
    /// invalid JSON, incompatible schema version, or schema mismatch.
    /// Source artifacts are rejected — the result must come from an action.
    ///
    /// See [Writing the validator](https://buck2.build/docs/rule_authors/validation/#writing-the-validator)
    /// in the Validations guide for the full schema reference and examples.
    validation_result: ValueOf<'v, StarlarkArtifactView<'v>>,

    /// If `True`, the validation is skipped by default and only runs when
    /// the user passes `--enable-optional-validations <name>`. Defaults to
    /// `False` (required).
    optional: bool,
}

starlark_complex_value!(pub(crate) StarlarkValidationSpec);

impl<'v> StarlarkValidationSpec<'v> {
    pub fn name(&self) -> &'v str {
        self.name.as_str()
    }

    pub fn validation_result(&self) -> &'v dyn StarlarkInputArtifactLike<'v> {
        self.validation_result.unpack().as_input_artifact_like()
    }

    pub fn optional(&self) -> bool {
        self.optional
    }
}

impl<'v> Display for StarlarkValidationSpec<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "ValidationSpec(name={}, validation_result=", self.name)?;
        Display::fmt(&self.validation_result, f)?;
        write!(f, ")")
    }
}

fn validate_validation_spec<'v>(spec: &StarlarkValidationSpec<'v>) -> buck2_error::Result<()> {
    if spec.name().is_empty() {
        return Err(ValidationSpecError::EmptyName.into());
    }
    let artifact = match spec.validation_result().get_bound_artifact() {
        Ok(bound_artifact) => bound_artifact,
        Err(e) => {
            return Err(e.context("Validation result artifact should be bound."));
        }
    };
    if artifact.is_source() {
        return Err(ValidationSpecError::ValidationResultIsSourceArtifact.into());
    }
    Ok(())
}

starlark::methods_static!(VALIDATION_SPEC_METHODS = validation_spec_methods);

#[starlark_value(type = "ValidationSpec")]
impl<'v> StarlarkValue<'v> for StarlarkValidationSpec<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(VALIDATION_SPEC_METHODS.methods())
    }
}

#[starlark_module]
fn validation_spec_methods(builder: &mut MethodsBuilder) {
    #[starlark(attribute)]
    /// Unique name identifying this validation within its `ValidationInfo`.
    fn name<'v>(
        this: &'v StarlarkValidationSpec,
        heap: Heap<'v>,
    ) -> starlark::Result<StringValue<'v>> {
        Ok(heap.alloc_str_intern(this.name()))
    }

    #[starlark(attribute)]
    /// Whether this validation is skipped by default (only run when explicitly
    /// enabled via `--enable-optional-validations <name>`).
    fn optional<'v>(this: &'v StarlarkValidationSpec) -> starlark::Result<bool> {
        Ok(this.optional())
    }

    #[starlark(attribute)]
    /// Build artifact produced by the validator. After the producing action
    /// runs, Buck2 reads the file as UTF-8 JSON and uses its contents to
    /// decide pass/fail.
    ///
    /// Expected shape:
    ///
    /// ```json
    /// {
    ///   "version": 1,
    ///   "data": {
    ///     "status": "success",
    ///     "message": "optional human-readable detail"
    ///   }
    /// }
    /// ```
    ///
    /// - `version` (int, required): schema version. Currently `1`.
    /// - `data.status` (string, required): `"success"` or `"failure"`.
    /// - `data.message` (string, optional): shown to the user; supply on
    ///   failure so the diagnostic is actionable.
    ///
    /// Additional fields outside the required ones are tolerated and ignored
    /// by Buck2 — both at the top level (alongside `version` / `data`) and
    /// inside `data` (alongside `status` / `message`). This is a deliberate
    /// extension point: attach debug or diagnostic info (e.g.
    /// `data.duration_ms`, `data.tool_version`, links to a build dashboard)
    /// that you want carried with the verdict.
    ///
    /// Buck2 surfaces three distinct errors if the file does not conform:
    /// invalid JSON, incompatible schema version, or schema mismatch. Source
    /// artifacts are rejected — the result must come from an action.
    ///
    /// See [Writing the validator](https://buck2.build/docs/rule_authors/validation/#writing-the-validator)
    /// in the Validations guide for end-to-end examples.
    fn validation_result<'v>(
        this: &'v StarlarkValidationSpec,
    ) -> starlark::Result<StarlarkArtifact> {
        Ok(this.validation_result().get_bound_starlark_artifact()?)
    }
}

#[starlark_module]
pub fn register_validation_spec(builder: &mut GlobalsBuilder) {
    #[starlark(as_type = StarlarkValidationSpec<'static>)]
    fn ValidationSpec<'v>(
        #[starlark(require = named)] name: StringValue<'v>,
        #[starlark(require = named)] validation_result: ValueOf<'v, StarlarkArtifactView<'v>>,
        #[starlark(require = named, default = false)] optional: bool,
    ) -> starlark::Result<StarlarkValidationSpec<'v>> {
        let result = StarlarkValidationSpec {
            name,
            validation_result,
            optional,
        };
        validate_validation_spec(&result)?;
        Ok(result)
    }
}
