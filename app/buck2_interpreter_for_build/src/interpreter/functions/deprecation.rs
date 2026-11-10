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
enum DeprecationError {
    #[error("Deprecation logged from Starlark: {category}: {message}\n{call_stack}")]
    WithStack {
        category: String,
        message: String,
        call_stack: String,
    },
    #[error("Deprecation logged from Starlark: {category}: {message}")]
    WithoutStack { category: String, message: String },
    #[error("Deprecation category must start with `starlark_`, got: `{0}`")]
    InvalidCategory(String),
    #[error("`owner` must be the unixname of the person driving the removal")]
    MissingOwner,
}

fn validate_category(category: &str) -> starlark::Result<()> {
    if !category.starts_with("starlark_") {
        return Err(buck2_error::Error::from(DeprecationError::InvalidCategory(
            category.to_owned(),
        ))
        .into());
    }

    validate_logview_category(category).map_err(starlark::Error::from)?;

    Ok(())
}

fn record_deprecation(
    category: &str,
    message: String,
    owner: &str,
    started: &str,
    stack: bool,
    eval: &mut Evaluator<'_, '_, '_>,
) -> starlark::Result<NoneType> {
    validate_category(category)?;

    let err = if stack {
        DeprecationError::WithStack {
            category: category.to_owned(),
            message,
            call_stack: eval.call_stack().to_string(),
        }
        .into()
    } else {
        DeprecationError::WithoutStack {
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
                owner: Cow::Owned(owner.to_owned()),
                started: Cow::Owned(started.to_owned()),
            },
            ..Default::default()
        },
    )?;
    Ok(NoneType)
}

#[starlark_module]
pub(crate) fn register_deprecation(builder: &mut GlobalsBuilder) {
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
            return Err(buck2_error::Error::from(DeprecationError::MissingOwner).into());
        }
        record_deprecation(category, message, owner, started, stack, eval)
    }
}

#[cfg(test)]
mod tests {
    use starlark::assert::Assert;

    use crate::interpreter::functions::deprecation::register_deprecation;

    #[test]
    fn test_deprecation_category_needs_prefix() {
        let mut a = Assert::new();
        a.globals_add(register_deprecation);
        for category in ["no_starlark_prefix", ""] {
            a.fail(
                &format!(
                    "deprecation('{category}', 'Blah', owner = 'jakobdegen', started = '2026-11-02')"
                ),
                "must start with `starlark_`",
            );
        }
    }

    #[test]
    fn test_deprecation_category_is_snake_case() {
        let mut a = Assert::new();
        a.globals_add(register_deprecation);
        for category in ["starlark_", "starlark_has__double", "starlark_has_1number"] {
            a.fail(
                &format!(
                    "deprecation('{category}', 'Blah', owner = 'jakobdegen', started = '2026-11-02')"
                ),
                "must be lower_snake_case",
            );
        }
    }

    #[test]
    fn test_deprecation_requires_owner_and_started() {
        let mut a = Assert::new();
        a.globals_add(register_deprecation);
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
        a.globals_add(register_deprecation);
        a.fail(
            "deprecation('starlark_going_away', 'Blah', owner = 'jakobdegen', started = 'yesterday')",
            "yesterday",
        );
    }
}
