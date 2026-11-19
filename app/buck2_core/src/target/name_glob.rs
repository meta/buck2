/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_error::internal_error;
use buck2_util::arc_str::ThinArcStr;
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

use crate::target::name::EQ_SIGN_SUBST;
use crate::target::name::TARGET_NAME_VALID_CHARS_SET;
use crate::target::name::TargetNameRef;

/// Glob-like matcher for target names.
///
/// `*` is the only wildcard. All other characters must be valid target-name
/// characters (ASCII-only, enforced by `TARGET_NAME_VALID_CHARS_SET`).
#[derive(
    Debug,
    Clone,
    Dupe,
    Eq,
    PartialEq,
    Hash,
    StrongHash,
    Allocative,
    Pagable,
    derive_more::Display
)]
#[display("{pattern}")]
pub struct TargetNameGlob {
    pattern: ThinArcStr,
}

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum TargetNameGlobError {
    #[error("Target name glob pattern cannot be empty")]
    Empty,
    #[error("Target name glob pattern matches every target; use `\"PUBLIC\"` instead, got `{0}`")]
    MatchesEverything(String),
    #[error(
        "Target name glob pattern can only contain target name characters and `*` wildcards, got `{0}`"
    )]
    InvalidChars(String),
    #[error("Target name glob pattern cannot contain target-name escape pattern `{0}`: `{1}`")]
    EscapePattern(&'static str, String),
    #[error("Target name glob pattern cannot be equal to `...`")]
    DotDotDot,
}

impl TargetNameGlob {
    pub fn try_new(pattern: String) -> buck2_error::Result<Self> {
        Self::validate(&pattern)?;
        Ok(Self {
            pattern: ThinArcStr::from(pattern.as_str()),
        })
    }

    fn validate(pattern: &str) -> buck2_error::Result<()> {
        if pattern.is_empty() {
            return Err(TargetNameGlobError::Empty.into());
        }

        // Reject a single `*` (or a run of only `*`s) since it is equivalent to
        // `"PUBLIC"` and should use that spelling instead.
        if pattern.chars().all(|c| c == '*') {
            return Err(TargetNameGlobError::MatchesEverything(pattern.to_owned()).into());
        }

        if !pattern
            .chars()
            .all(|c| c.is_ascii() && (TARGET_NAME_VALID_CHARS_SET.contains(c as u8) || c == '*'))
        {
            return Err(TargetNameGlobError::InvalidChars(pattern.to_owned()).into());
        }
        if pattern.contains(EQ_SIGN_SUBST) {
            return Err(
                TargetNameGlobError::EscapePattern(EQ_SIGN_SUBST, pattern.to_owned()).into(),
            );
        }
        if pattern == "..." {
            return Err(TargetNameGlobError::DotDotDot.into());
        }

        Ok(())
    }

    pub fn matches(&self, name: &TargetNameRef) -> buck2_error::Result<bool> {
        let pattern = self.pattern.as_str();
        let name = name.as_str();

        if !pattern.contains('*') {
            return Ok(pattern == name);
        }

        let mut parts = pattern.split('*').filter(|p| !p.is_empty()).peekable();
        let mut remaining = name;

        if !pattern.starts_with("*") {
            let first = parts.next().ok_or_else(|| {
                internal_error!(
                    "All-wildcard pattern `{}` should have been rejected by validation",
                    pattern
                )
            })?;
            let Some(rest) = remaining.strip_prefix(first) else {
                return Ok(false);
            };
            remaining = rest;
        }

        let ends_with_star = pattern.ends_with("*");
        while let Some(part) = parts.next() {
            if parts.peek().is_none() && !ends_with_star {
                return Ok(remaining.ends_with(part));
            }

            let Some(pos) = remaining.find(part) else {
                return Ok(false);
            };
            remaining = &remaining[pos + part.len()..];
        }

        Ok(true)
    }

    pub fn pattern(&self) -> &str {
        self.pattern.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use crate::target::name::TargetNameRef;
    use crate::target::name_glob::TargetNameGlob;

    #[test]
    fn test_target_name_glob_star() {
        // Single `*` is rejected; use "PUBLIC" instead.
        assert!(TargetNameGlob::try_new("*".to_owned()).is_err());
    }

    #[test]
    fn test_target_name_glob_prefix() {
        let glob = TargetNameGlob::try_new("foo*".to_owned()).unwrap();
        assert!(glob.matches(&TargetNameRef::unchecked_new("foo")).unwrap());
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("foobar"))
                .unwrap()
        );
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("foo123"))
                .unwrap()
        );
        assert!(!glob.matches(&TargetNameRef::unchecked_new("bar")).unwrap());
        assert!(
            !glob
                .matches(&TargetNameRef::unchecked_new("barfoo"))
                .unwrap()
        );
    }

    #[test]
    fn test_target_name_glob_suffix() {
        let glob = TargetNameGlob::try_new("*-test".to_owned()).unwrap();
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("foo-test"))
                .unwrap()
        );
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("bar-test"))
                .unwrap()
        );
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("-test"))
                .unwrap()
        );
        assert!(!glob.matches(&TargetNameRef::unchecked_new("test")).unwrap());
        assert!(
            !glob
                .matches(&TargetNameRef::unchecked_new("foo-test-bar"))
                .unwrap()
        );
    }

    #[test]
    fn test_target_name_glob_contains() {
        let glob = TargetNameGlob::try_new("*foo*".to_owned()).unwrap();
        assert!(glob.matches(&TargetNameRef::unchecked_new("foo")).unwrap());
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("foobar"))
                .unwrap()
        );
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("barfoo"))
                .unwrap()
        );
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("barfoobar"))
                .unwrap()
        );
        assert!(!glob.matches(&TargetNameRef::unchecked_new("bar")).unwrap());
    }

    #[test]
    fn test_target_name_glob_exact() {
        let glob = TargetNameGlob::try_new("foo".to_owned()).unwrap();
        assert!(glob.matches(&TargetNameRef::unchecked_new("foo")).unwrap());
        assert!(
            !glob
                .matches(&TargetNameRef::unchecked_new("foobar"))
                .unwrap()
        );
        assert!(!glob.matches(&TargetNameRef::unchecked_new("bar")).unwrap());
    }

    #[test]
    fn test_target_name_glob_multiple_stars() {
        let glob = TargetNameGlob::try_new("a*b*c".to_owned()).unwrap();
        assert!(glob.matches(&TargetNameRef::unchecked_new("abc")).unwrap());
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("a1b2c"))
                .unwrap()
        );
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("aXXbYYc"))
                .unwrap()
        );
        assert!(!glob.matches(&TargetNameRef::unchecked_new("ac")).unwrap());
        assert!(!glob.matches(&TargetNameRef::unchecked_new("ab")).unwrap());
    }

    #[test]
    fn test_target_name_glob_backtracks_from_latest_star() {
        let glob = TargetNameGlob::try_new("*ab*cd".to_owned()).unwrap();
        assert!(
            glob.matches(&TargetNameRef::unchecked_new("zzabyycd"))
                .unwrap()
        );
        assert!(
            !glob
                .matches(&TargetNameRef::unchecked_new("zzabyyce"))
                .unwrap()
        );
    }

    #[test]
    fn test_target_name_glob_repeated_suffix_non_match() {
        let glob = TargetNameGlob::try_new("*aaaaab".to_owned()).unwrap();
        assert!(
            !glob
                .matches(&TargetNameRef::unchecked_new("aaaaaaaaaaaaaaaa"))
                .unwrap()
        );
    }

    #[test]
    fn test_target_name_glob_invalid_chars() {
        assert!(TargetNameGlob::try_new("foo?".to_owned()).is_err());
        assert!(TargetNameGlob::try_new("foo[bar]".to_owned()).is_err());
        assert!(TargetNameGlob::try_new("foo[0-9]".to_owned()).is_err());
        assert!(TargetNameGlob::try_new("foo bar".to_owned()).is_err());
        assert!(TargetNameGlob::try_new("foo\"bar".to_owned()).is_err());
        assert!(TargetNameGlob::try_new("foo_eqsb_bar".to_owned()).is_err());
        assert!(TargetNameGlob::try_new("...".to_owned()).is_err());
    }

    #[test]
    fn test_target_name_glob_empty() {
        assert!(TargetNameGlob::try_new("".to_owned()).is_err());
    }

    #[test]
    fn test_target_name_glob_consecutive_stars() {
        // All-`*` patterns are rejected; use "PUBLIC" instead.
        assert!(TargetNameGlob::try_new("***".to_owned()).is_err());

        let surrounded = TargetNameGlob::try_new("***foo***".to_owned()).unwrap();
        assert!(
            surrounded
                .matches(&TargetNameRef::unchecked_new("foo"))
                .unwrap()
        );
        assert!(
            surrounded
                .matches(&TargetNameRef::unchecked_new("xfooy"))
                .unwrap()
        );
        assert!(
            !surrounded
                .matches(&TargetNameRef::unchecked_new("bar"))
                .unwrap()
        );

        let middle = TargetNameGlob::try_new("a***b".to_owned()).unwrap();
        assert!(middle.matches(&TargetNameRef::unchecked_new("ab")).unwrap());
        assert!(
            middle
                .matches(&TargetNameRef::unchecked_new("aXb"))
                .unwrap()
        );
        assert!(!middle.matches(&TargetNameRef::unchecked_new("a")).unwrap());
    }

    #[test]
    fn test_target_name_glob_error_messages() {
        // Each invalid input must produce the matching error message so
        // refactors can't silently re-route variants.
        fn assert_err_contains(input: &str, expected_substring: &str) {
            let err = TargetNameGlob::try_new(input.to_owned())
                .err()
                .unwrap_or_else(|| panic!("expected `{input}` to be rejected"));
            let msg = format!("{err:#}");
            assert!(
                msg.contains(expected_substring),
                "expected error for `{input}` to contain {expected_substring:?}, got: {msg}"
            );
        }

        // Empty
        assert_err_contains("", "cannot be empty");
        // MatchesEverything (all-`*` patterns)
        assert_err_contains("*", "matches every target");
        assert_err_contains("***", "matches every target");
        // InvalidChars
        assert_err_contains("foo?", "can only contain target name characters");
        assert_err_contains("foo bar", "can only contain target name characters");
        // EscapePattern
        assert_err_contains("foo_eqsb_bar", "escape pattern");
        // DotDotDot
        assert_err_contains("...", "cannot be equal to `...`");
    }
}
