/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;

use buck2_core::error::SoftErrorKind;
use buck2_core::error::StructuredErrorOptions;
use buck2_core::error::handle_soft_error;
use buck2_core::error::validate_logview_category;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::none::NoneType;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum SoftErrorError {
    #[error("Error produced by Starlark: {category}: {message}\n{call_stack}")]
    StarlarkSoftError {
        category: String,
        message: String,
        call_stack: String,
    },
    #[error("Error produced by Starlark: {category}: {message}")]
    StarlarkSoftErrorNoStack { category: String, message: String },
    #[error(
        "soft_error originated from starlark should have category starting with `starlark_`, got: `{0}`"
    )]
    InvalidCategory(String),
    #[error("`owner` must be the unixname of the person driving the removal")]
    MissingOwner,
}

fn validate_category(category: &str) -> starlark::Result<()> {
    if !category.starts_with("starlark_") {
        return Err(
            buck2_error::Error::from(SoftErrorError::InvalidCategory(category.to_owned())).into(),
        );
    }

    validate_logview_category(category).map_err(starlark::Error::from)?;

    Ok(())
}

fn record_deprecation(
    category: &str,
    message: String,
    owner: Option<&str>,
    started: Option<&str>,
    stack: bool,
    eval: &mut Evaluator<'_, '_, '_>,
) -> starlark::Result<NoneType> {
    validate_category(category)?;

    let err = if stack {
        SoftErrorError::StarlarkSoftError {
            category: category.to_owned(),
            message,
            call_stack: eval.call_stack().to_string(),
        }
        .into()
    } else {
        SoftErrorError::StarlarkSoftErrorNoStack {
            category: category.to_owned(),
            message,
        }
        .into()
    };

    handle_soft_error(
        category,
        err,
        (file!(), line!(), column!()),
        StructuredErrorOptions {
            kind: SoftErrorKind::Deprecation {
                owner: owner.map(|owner| Cow::Owned(owner.to_owned())),
                started: started.map(|started| Cow::Owned(started.to_owned())),
            },
            ..Default::default()
        },
    )?;
    Ok(NoneType)
}

#[starlark_module]
pub(crate) fn register_soft_error(builder: &mut GlobalsBuilder) {
    /// Log a use of a deprecated codepath.
    ///
    /// Invocations of this function are logged by buck2's standard logging facilities. The first
    /// argument identifies the deprecation and should be a lowercase, snake-case ascii string
    /// literal beginning with `starlark_`. The second argument is an arbitrary message that can
    /// contain any additional details you want to log.
    ///
    /// At Meta, the results make their way into `logview/buck2`.
    ///
    /// In a monorepo, you do not typically have to use this. Instead, if you want to delete
    /// something, just delete it; codesearch and otherwise CI are normally sufficient to find uses.
    /// This is for the rare instances of exceptions to that rule.
    ///
    /// This function does not result in any deprecation warnings becoming visible to users or
    /// callers. There is no mechanism for doing so, as it results in externalizing the costs of
    /// migrations.
    ///
    /// You must pass an owner and a started date, typically your unixname and the day you added the
    /// call to this function. These fields are not used for anything, they are there to provide
    /// gentle social encouragement to complete your migrations promptly.
    ///
    /// ```python
    /// deprecation(
    ///     "starlark_rule_is_too_long",
    ///     "Length of property exceeds 100 characters in " + repr(ctx.label),
    ///     owner = "yournamehere",
    ///     started = "2026-10-26",
    /// )
    /// ```
    fn deprecation<'v>(
        #[starlark(require = pos)] category: &str,
        #[starlark(require = pos)] message: String,
        #[starlark(require = named)] owner: &str,
        #[starlark(require = named)] started: &str,
        #[starlark(require = named, default = true)] stack: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        if owner.is_empty() {
            return Err(buck2_error::Error::from(SoftErrorError::MissingOwner).into());
        }
        record_deprecation(category, message, Some(owner), Some(started), stack, eval)
    }

    /// Will soon be removed in favor of `deprecation`.
    fn soft_error<'v>(
        #[starlark(require = pos)] category: &str,
        #[starlark(require = pos)] message: String,
        #[starlark(require = named)] stack: Option<bool>,
        #[starlark(require = named)] quiet: Option<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        // A deprecation is never shown, so `quiet` has nothing to say; it stays accepted until
        // the callers in the repo have dropped it, since they and buck2 do not release together.
        let _ = quiet;
        record_deprecation(category, message, None, None, stack.unwrap_or(true), eval)
    }

    /// Validate that a soft error category is valid for use from Starlark.
    /// The category must start with `starlark_` and be `lower_snake_case`
    /// (only `a-z` and `_`, no consecutive underscores, must start and end with a letter).
    /// Raises an error if the category is invalid, returns `None` on success.
    fn validate_soft_error_category(
        #[starlark(require = pos)] category: &str,
    ) -> starlark::Result<NoneType> {
        validate_category(category)?;
        Ok(NoneType)
    }
}

#[cfg(test)]
mod tests {
    use starlark::assert::Assert;

    use crate::interpreter::functions::soft_error::register_soft_error;

    #[test]
    fn test_validate_soft_error_category_valid() {
        let mut a = Assert::new();
        a.globals_add(register_soft_error);
        a.is_true("validate_soft_error_category('starlark_valid_category') == None");
    }

    #[test]
    fn test_validate_soft_error_category_missing_prefix() {
        let mut a = Assert::new();
        a.globals_add(register_soft_error);
        a.fail(
            "validate_soft_error_category('no_starlark_prefix')",
            "should have category starting with `starlark_`",
        );
    }

    #[test]
    fn test_validate_soft_error_category_ends_with_underscore() {
        let mut a = Assert::new();
        a.globals_add(register_soft_error);
        a.fail(
            "validate_soft_error_category('starlark_')",
            "must be lower_snake_case",
        );
    }

    #[test]
    fn test_validate_soft_error_category_consecutive_underscores() {
        let mut a = Assert::new();
        a.globals_add(register_soft_error);
        a.fail(
            "validate_soft_error_category('starlark_has__double')",
            "must be lower_snake_case",
        );
    }

    #[test]
    fn test_validate_soft_error_category_contains_digit() {
        let mut a = Assert::new();
        a.globals_add(register_soft_error);
        a.fail(
            "validate_soft_error_category('starlark_has_1number')",
            "must be lower_snake_case",
        );
    }

    #[test]
    fn test_validate_soft_error_category_empty() {
        let mut a = Assert::new();
        a.globals_add(register_soft_error);
        a.fail(
            "validate_soft_error_category('')",
            "should have category starting with `starlark_`",
        );
    }

    #[test]
    fn test_deprecation_requires_owner_and_started() {
        let mut a = Assert::new();
        a.globals_add(register_soft_error);
        a.fail(
            "deprecation('starlark_going_away', 'Blah', started = '2026-10-26')",
            "owner",
        );
        a.fail(
            "deprecation('starlark_going_away', 'Blah', owner = 'jakobdegen')",
            "started",
        );
        a.fail(
            "deprecation('starlark_going_away', 'Blah', owner = '', started = '2026-10-26')",
            "unixname",
        );
    }

    #[test]
    fn test_deprecation_start_is_a_day() {
        let mut a = Assert::new();
        a.globals_add(register_soft_error);
        a.fail(
            "deprecation('starlark_going_away', 'Blah', owner = 'jakobdegen', started = 'yesterday')",
            "yesterday",
        );
    }
}
