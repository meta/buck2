/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_hash::BuckMutSet;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::ListType;

use crate as buck2_build_api;
use crate::interpreter::rule_defs::validation_spec::StarlarkValidationSpec;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum ValidationInfoError {
    #[error("Multiple specs with same name `{0}` which is not allowed.")]
    SpecsWithDuplicateName(String),
    #[error("`ValidationInfo` should contain at least one validation.")]
    ValidationSpecsEmpty,
}

/// Provider declaring how a target should be validated.
///
/// When a target carrying `ValidationInfo` is reachable via a transitive
/// dependency edge from a target requested on the command line (e.g.
/// `buck2 build`, `buck2 test`), Buck2 schedules every `ValidationSpec`
/// it carries before the requested action is considered complete. A
/// failed required validation causes the build to fail; an optional
/// validation is skipped unless the user opts in via
/// `--enable-optional-validations <name>`.
///
/// Validations run in parallel with the build of the requested target —
/// they only have to finish before Buck2 reports success.
///
/// Constraints enforced at construction / freezing time:
/// - `validations` must be a non-empty list of `ValidationSpec` values.
/// - Spec names must be unique within the provider.
///
/// Example:
/// ```python
/// def _my_rule_impl(ctx):
///     report = ctx.actions.declare_output("validation.json")
///     ctx.actions.run(
///         cmd_args("validator", "--out", report.as_output(), ctx.attrs.src),
///         category = "my_validation",
///     )
///     return [
///         DefaultInfo(default_output = ctx.attrs.src),
///         ValidationInfo(validations = [
///             ValidationSpec(name = "schema_check", validation_result = report),
///         ]),
///     ]
/// ```
///
/// See the [Validations guide](https://buck2.build/docs/rule_authors/validation/)
/// for the end-to-end story.
#[internal_provider(validation_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[freeze(validator = validate_validation_info)]
#[repr(transparent)]
pub struct ValidationInfo<'v> {
    /// Non-empty list of `ValidationSpec` values, each representing a single
    /// validation. Spec names must be unique within this provider.
    ///
    /// See the [Validations guide](https://buck2.build/docs/rule_authors/validation/)
    /// for how to declare validations end-to-end and write the validator
    /// action that produces each spec's `validation_result`.
    validations: ValueOf<'v, ListType<ValueTyped<'v, StarlarkValidationSpec<'v>>>>,
}

fn validate_validation_info<'v>(info: &ValidationInfo<'v>) -> buck2_error::Result<()> {
    let mut spec_names = BuckMutSet::default();
    for spec in info.validations.iter() {
        let name = spec?.unpack().as_ref().name();
        if !spec_names.insert(name) {
            return Err(ValidationInfoError::SpecsWithDuplicateName(name.to_owned()).into());
        }
    }
    if spec_names.is_empty() {
        return Err(ValidationInfoError::ValidationSpecsEmpty.into());
    }
    Ok(())
}

#[starlark_module]
fn validation_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = ValidationInfo<'static>)]
    fn ValidationInfo<'v>(
        #[starlark(require = named)] validations: ValueOf<
            'v,
            ListType<ValueTyped<'v, StarlarkValidationSpec<'v>>>,
        >,
    ) -> starlark::Result<ValidationInfo<'v>> {
        let result = ValidationInfo { validations };
        validate_validation_info(&result)?;
        Ok(result)
    }
}

/// A `ValidationInfo` kept alive by its owning frozen heap; usable across threads and awaits.
pub type OwnedValidationInfo = OwnedFrozen<ValueTyped<'static, ValidationInfo<'static>>>;

impl<'v> ValidationInfo<'v> {
    pub fn validations(
        &self,
    ) -> impl Iterator<Item = buck2_error::Result<&'v StarlarkValidationSpec<'v>>> + 'v {
        self.validations
            .iter()
            .map(|spec| Ok(spec?.unpack().as_ref()))
    }
}
