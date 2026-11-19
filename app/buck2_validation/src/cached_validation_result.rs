/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use dupe::Dupe;
use pagable::Pagable;

use crate::validator_api::ValidationResult;
use crate::validator_api::ValidationStatus;

/// Result of running a validation, cached in DICE.
#[derive(Clone, Dupe, Allocative, PartialEq, Pagable)]
pub(crate) struct CachedValidationResult(pub(crate) Arc<CachedValidationResultData>);

#[derive(Allocative, PartialEq, Pagable)]
pub(crate) enum CachedValidationResultData {
    Success,
    Failure(ValidationFailedUserFacingError),
}

#[derive(buck2_error::Error, Debug, PartialEq, Allocative, Clone, Pagable)]
#[buck2(input)]
#[error(
    "Validation for `{target}` failed:\n\n{}\n\nFull validation result is located at: `{result_path}`", self.rendered_message()
)]
pub(crate) struct ValidationFailedUserFacingError {
    target: BaseDeferredKey,
    short_message: Option<String>,
    result_path: AbsNormPathBuf,
}

impl ValidationFailedUserFacingError {
    pub(crate) fn rendered_message(&self) -> &str {
        self.short_message
            .as_deref()
            .unwrap_or("Diagnostic message is missing from validation result")
    }
}

impl CachedValidationResult {
    pub(crate) fn new(
        parsed_result: ValidationResult,
        target: BaseDeferredKey,
        validation_result_path: AbsNormPathBuf,
    ) -> CachedValidationResult {
        let data = match parsed_result {
            ValidationResult {
                status: ValidationStatus::Success,
                ..
            } => CachedValidationResultData::Success,
            ValidationResult {
                status: ValidationStatus::Failure,
                message,
            } => CachedValidationResultData::Failure(ValidationFailedUserFacingError::new(
                message,
                target,
                validation_result_path,
            )),
        };
        Self(Arc::new(data))
    }
}

impl ValidationFailedUserFacingError {
    pub(crate) fn new(
        message: Option<String>,
        target: BaseDeferredKey,
        validation_result_path: AbsNormPathBuf,
    ) -> Self {
        let short_message = message.map(|x| {
            const MAX_CACHED_LENGTH: usize = 2000;
            // Shortened message as we don't want to store too much data in DICE
            shorten_message(x, MAX_CACHED_LENGTH)
        });
        Self {
            target,
            short_message,
            result_path: validation_result_path,
        }
    }
}

/// If original message exceeds the limit, cut it and add an ellipsis.
fn shorten_message(mut message: String, max_bytes: usize) -> String {
    if message.len() > max_bytes {
        let new_length = message.floor_char_boundary(max_bytes);
        message.truncate(new_length);
        message.push_str("...");
    }
    message
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use buck2_core::configuration::data::ConfigurationData;
    #[cfg(unix)]
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;

    use super::*;

    #[test]
    fn test_shorten_message() {
        assert_eq!(shorten_message("Hello World".to_owned(), 5), "Hello...");
        assert_eq!(shorten_message("Hello World".to_owned(), 11), "Hello World");
        assert_eq!(
            shorten_message("Hello World".to_owned(), 100),
            "Hello World"
        );
        assert_eq!(shorten_message("Привет, мир".to_owned(), 7), "При...");
        assert_eq!(shorten_message("Привет, мир".to_owned(), 12), "Привет...");
        assert_eq!(shorten_message("Привет, мир".to_owned(), 22), "Привет, мир");
        assert_eq!(shorten_message("你好世界".to_owned(), 5), "你...");
        assert_eq!(shorten_message("你好世界".to_owned(), 8), "你好...");
        assert_eq!(shorten_message("你好世界".to_owned(), 16), "你好世界");
    }

    #[cfg(unix)]
    #[test]
    fn test_error_rendering() -> buck2_error::Result<()> {
        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());
        let path = AbsNormPathBuf::from("/my/path/to/validation/result".to_owned())?;
        assert_eq!(
            format!(
                "{}",
                ValidationFailedUserFacingError::new(
                    None,
                    BaseDeferredKey::TargetLabel(target.dupe()),
                    path.clone(),
                )
            ),
            r#"Validation for `cell//pkg:foo (<testing>#c617aa931f2642de)` failed:

Diagnostic message is missing from validation result

Full validation result is located at: `/my/path/to/validation/result`"#
        );
        assert_eq!(
            format!(
                "{}",
                ValidationFailedUserFacingError::new(
                    Some("Here is my diagnostic message".to_owned()),
                    BaseDeferredKey::TargetLabel(target),
                    path,
                )
            ),
            r#"Validation for `cell//pkg:foo (<testing>#c617aa931f2642de)` failed:

Here is my diagnostic message

Full validation result is located at: `/my/path/to/validation/result`"#
        );
        Ok(())
    }
}
