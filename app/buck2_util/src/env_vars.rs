/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::env::VarError;
use std::sync::LazyLock;

use regex::Regex;

/// Replace occurrences of `$FOO` in a string with the value of the env var `$FOO`.
/// Returns an error if any referenced variable is not set.
pub fn substitute_env_vars(s: &str) -> buck2_error::Result<String> {
    substitute_env_vars_impl(s, |v| std::env::var(v))
}

fn substitute_env_vars_impl(
    s: &str,
    getter: impl Fn(&str) -> Result<String, VarError>,
) -> buck2_error::Result<String> {
    static ENV_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new("\\$[a-zA-Z_][a-zA-Z_0-9]*").expect("hard-coded regex is valid")
    });

    // `Replacer` cannot fail, so capture the first lookup error and surface it after replacement.
    let mut err = None;
    let out = ENV_REGEX.replace_all(s, |caps: &regex::Captures| {
        let matched = &caps[0];
        match getter(&matched[1..]) {
            Ok(val) => val,
            Err(e) => {
                err.get_or_insert_with(|| {
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Environment,
                        "Error substituting `{}`: {}",
                        matched,
                        e
                    )
                });
                String::new()
            }
        }
    });

    match err {
        Some(e) => Err(e),
        None => Ok(out.into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::env::VarError;

    use super::*;

    #[test]
    fn test_substitute_env_vars() {
        let getter = |s: &str| match s {
            "FOO" => Ok("foo_value".to_owned()),
            "BAR" => Ok("bar_value".to_owned()),
            "BAZ" => Err(VarError::NotPresent),
            _ => panic!("Unexpected"),
        };

        assert_eq!(
            substitute_env_vars_impl("$FOO", getter).unwrap(),
            "foo_value"
        );
        assert_eq!(
            substitute_env_vars_impl("$FOO$BAR", getter).unwrap(),
            "foo_valuebar_value"
        );
        assert_eq!(
            substitute_env_vars_impl("some$FOO.bar", getter).unwrap(),
            "somefoo_value.bar"
        );
        assert_eq!(substitute_env_vars_impl("foo", getter).unwrap(), "foo");
        assert_eq!(substitute_env_vars_impl("FOO", getter).unwrap(), "FOO");
        assert!(substitute_env_vars_impl("$FOO$BAZ", getter).is_err());
    }
}
