/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! This module defines the API between validators and Buck2 by specifying
//! the schema for a JSON file that represents the result of a validator run.

use serde::Deserialize;
use serde_json::Value;

const CURRENT_VERSION: i32 = 1;

/// The documented layout of a validation result file.
#[derive(Deserialize, Debug)]
struct ValidationStaticSchema {
    #[allow(dead_code, reason = "checked before the schema is deserialized")]
    version: i32,
    data: ValidationResult,
}

#[derive(Deserialize, Debug, PartialEq)]
pub(crate) enum ValidationStatus {
    #[serde(alias = "success")]
    Success,
    #[serde(alias = "failure")]
    Failure,
}

#[derive(Deserialize, Debug, PartialEq)]
pub(crate) struct ValidationResult {
    pub status: ValidationStatus,
    pub message: Option<String>,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ValidationApiError {
    #[error("Validation result should contain valid JSON.")]
    InvalidJson {
        #[source]
        error: serde_json::Error,
    },
    #[error("Incompatible version of validation result `{0}`, expected `{1}`.")]
    IncompatibleVersion(i64, i32),
    #[error("JSON content doesn't match schema.")]
    JsonNotMatchingSchema {
        #[source]
        error: serde_json::Error,
    },
}

pub(crate) fn parse_validation_result(content: &str) -> buck2_error::Result<ValidationResult> {
    let json: Value = match serde_json::from_str(content) {
        Ok(x) => x,
        Err(error) => return Err((ValidationApiError::InvalidJson { error }).into()),
    };
    // Checked before the schema, so that a file written for another version is reported as such
    // whatever its layout is. An absent or ill-typed version fails the schema check below.
    if let Some(version) = json.get("version").and_then(Value::as_i64)
        && version != i64::from(CURRENT_VERSION)
    {
        return Err(ValidationApiError::IncompatibleVersion(version, CURRENT_VERSION).into());
    }
    match serde_json::from_value::<ValidationStaticSchema>(json) {
        Ok(result) => Ok(result.data),
        Err(error) => Err((ValidationApiError::JsonNotMatchingSchema { error }).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_broken_json() {
        let json = r#"
            {
                "version": 1
        "#;
        let error = parse_validation_result(json).expect_err("Expected parsing to fail");

        assert!(
            error
                .to_string()
                .contains("Validation result should contain valid JSON.")
        )
    }

    #[test]
    fn test_parse_wrong_schema() {
        let json = r#"
            {
                "data": {
                    "foo": 1,
                    "bar": 2
                }
            }
        "#;
        let error = parse_validation_result(json).expect_err("Expected parsing to fail");
        assert!(
            error
                .to_string()
                .contains("JSON content doesn't match schema.")
        )
    }

    #[test]
    fn test_parse_valid_result() {
        let json = r#"
            {
                "version": 1,
                "data": {
                    "status": "failure",
                    "message": "something somewhere"
                }
            }
        "#;
        let expected = ValidationResult {
            status: ValidationStatus::Failure,
            message: Some("something somewhere".to_owned()),
        };
        assert_eq!(parse_validation_result(json).unwrap(), expected);
    }

    #[test]
    fn test_parse_invalid_version() {
        let json = r#"
            {
                "version": 2,
                "data": {
                    "foo": 1,
                    "bar": 2
                }
            }
        "#;
        let error = parse_validation_result(json).expect_err("Expected parsing to fail");
        assert!(
            error
                .to_string()
                .contains("Incompatible version of validation result")
        )
    }

    #[test]
    fn test_parse_no_matching_schema_with_valid_version() {
        let json = r#"
            {
                "version": 1,
                "data": {
                    "message": "something somewhere"
                }
            }
        "#;
        let error = parse_validation_result(json).expect_err("Expected parsing to fail");
        assert!(
            error
                .to_string()
                .contains("JSON content doesn't match schema.")
        )
    }

    fn parse_err(json: &str) -> String {
        format!(
            "{:#}",
            parse_validation_result(json).expect_err("expected an error")
        )
    }

    /// Valid JSON without a `version` is a schema mismatch, not invalid JSON.
    #[test]
    fn test_missing_version_is_a_schema_mismatch() {
        let e = parse_err(r#"{"data": {"status": "success"}}"#);
        assert!(e.contains("JSON content doesn't match schema."), "{e}");
    }

    /// A file written for another version is reported as an incompatible version whatever its
    /// layout is.
    #[test]
    fn test_other_version_without_data_is_an_incompatible_version() {
        let e = parse_err(r#"{"version": 2, "result": {"ok": true}}"#);
        assert!(
            e.contains("Incompatible version of validation result `2`, expected `1`."),
            "{e}"
        );
    }

    /// An ill-typed `version` or a missing `data` is a schema mismatch.
    #[test]
    fn test_ill_typed_version_or_missing_data_is_a_schema_mismatch() {
        for json in [
            r#"{"version": 1.0, "data": {"status": "success"}}"#,
            r#"{"version": "1", "data": {"status": "success"}}"#,
            r#"{"version": 1}"#,
        ] {
            let e = parse_err(json);
            assert!(e.contains("JSON content doesn't match schema."), "{e}");
        }
    }
}
