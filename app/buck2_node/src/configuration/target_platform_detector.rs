/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The target platform detector spec is used to define a mapping of `package-prefix` to `target platform target`.
//! When performing "target platform resolution", if a target doesn't specify a `default_target_platform`, buck
//! will fallback to the target platform detector to determine the default target platform for that target.
//!
//! This is encoded in the buckconfig value `parser.target_platform_detector_spec` and has the format:
//!   `kind1:matcher1->platform1 kind2:matcher2->platform2 ...`
//!
//! The only supported _kind_ currently is "target". The _platform_ must be a target literal that defines a configuration (for example,
//! a `platform()` target).
//!
//! The "target" kind only supports a _matcher_ that is a recursive target pattern (ex. `cell//lib/...`).
//!
//! An example supported mapping is: `target://foo/...->//:tgt target:cell//bar/...->//:tgt2 target:cell//bar/foo/...->//:tgt`.
//! This would map `//foo:x` to platform `//:tgt`, `cell//bar:x` to `//:tgt2` and `cell//bar/foo:x` also to `//:tgt2` (the mapping
//! for `cell//bar/foo/...` has no effect because buck will pick the first matching spec).

use allocative::Allocative;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::name::CellName;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::BuckErrorContext;
use pagable::Pagable;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum DetectorSpecParseError {
    #[error(
        "`target:` platform detector only supports a recursive pattern matcher (like `cell//package/...`) but got `{0}`"
    )]
    TargetKindRequiresRecursivePattern(String),
    #[error(
        "Error parsing target platform detector spec, expected format `<kind>:<matcher> <kind>:<matcher>`, got `{0}`"
    )]
    UnrecognizedFormat(String),
    #[error(
        "Unsupported target platform detector kind, supported kinds are <\"target\">, got `{0}`"
    )]
    UnsupportedKind(String),
}

#[derive(Debug, Eq, PartialEq, Allocative, Pagable)]
pub struct TargetPlatformDetector {
    detectors: Vec<(CellPath, TargetLabel)>,
}

impl TargetPlatformDetector {
    pub fn empty() -> Self {
        Self {
            detectors: Vec::new(),
        }
    }
    pub fn parse_spec(
        spec: &str,
        cell_name: CellName,
        cell_resolver: &CellResolver,
        cell_alias_resolver: &CellAliasResolver,
    ) -> buck2_error::Result<Self> {
        let mut detectors = Vec::new();
        for value in spec.split_whitespace() {
            match value.split_once(':') {
                Some(("target", value)) => match value.split_once("->") {
                    Some((matcher, target)) => {
                        let matcher_package =
                            match ParsedPattern::<TargetPatternExtra>::parse_precise(
                                matcher,
                                cell_name,
                                cell_resolver,
                                cell_alias_resolver,
                            )? {
                                buck2_core::pattern::pattern::ParsedPattern::Recursive(root) => {
                                    root
                                }
                                _ => {
                                    return Err(
                                        DetectorSpecParseError::TargetKindRequiresRecursivePattern(
                                            matcher.to_owned(),
                                        )
                                        .into(),
                                    );
                                }
                            };
                        let target = ParsedPattern::<TargetPatternExtra>::parse_precise(
                            target,
                            cell_name,
                            cell_resolver,
                            cell_alias_resolver,
                        )
                        .and_then(|x| x.as_target_label(target))
                        .buck_error_context("Error parsing target platform detector spec")?;
                        detectors.push((matcher_package, target))
                    }
                    None => {
                        return Err(
                            DetectorSpecParseError::UnrecognizedFormat(spec.to_owned()).into()
                        );
                    }
                },
                Some((kind, _)) => {
                    return Err(DetectorSpecParseError::UnsupportedKind(kind.to_owned()).into());
                }
                None => {
                    return Err(DetectorSpecParseError::UnrecognizedFormat(spec.to_owned()).into());
                }
            }
        }
        Ok(TargetPlatformDetector { detectors })
    }

    pub fn detect(&self, target: &TargetLabel) -> Option<&TargetLabel> {
        for (root, platform) in &self.detectors {
            if target.pkg().cell_name() == root.cell()
                && target.pkg().cell_relative_path().starts_with(root.path())
            {
                return Some(platform);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::cells::alias::NonEmptyCellAlias;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_hash::IntentionallyStdHashMap;

    use super::*;

    #[test]
    fn test_parse_errors() -> buck2_error::Result<()> {
        let cell_resolver = CellResolver::testing_with_names_and_paths_with_alias(
            &[
                (
                    CellName::testing_new("root"),
                    CellRootPathBuf::testing_new(""),
                ),
                (
                    CellName::testing_new("cell1"),
                    CellRootPathBuf::testing_new("cell1"),
                ),
            ],
            IntentionallyStdHashMap::from([(
                NonEmptyCellAlias::testing_new("alias1"),
                CellName::testing_new("cell1"),
            )]),
        );
        let cell_alias_resolver = cell_resolver.root_cell_cell_alias_resolver();

        let check_fails = |spec: &str| {
            if TargetPlatformDetector::parse_spec(
                spec,
                CellName::testing_new("root"),
                &cell_resolver,
                cell_alias_resolver,
            )
            .is_ok()
            {
                panic!("Expected spec `{spec}` to fail parsing, but it succeeded.")
            }
        };

        let check_good = |spec: &str| {
            TargetPlatformDetector::parse_spec(
                spec,
                CellName::testing_new("root"),
                &cell_resolver,
                cell_alias_resolver,
            )
            .unwrap_or_else(|_| panic!("Expected parsing `{spec}` to succeed."))
        };

        check_good("target://...->//:tgt");
        check_good("target://...->//:tgt target:alias1//...->alias1//:tgt");

        // all bad
        check_fails("xxxx");
        // bad kind
        check_fails("xyz://...->//:tgt");
        // missing pattern to match
        check_fails("target:->//:tgt");
        // match pattern not recursive
        check_fails("target://some:->//:tgt");
        // bad cell alias
        check_fails("target://...->missing//:tgt");
        // target not a literal
        check_fails("target://...->//...");
        // first spec good, second spec bad
        check_fails("target://...->//:tgt xyz");

        Ok(())
    }

    #[test]
    fn test_detect() -> buck2_error::Result<()> {
        let cell_resolver = CellResolver::testing_with_names_and_paths_with_alias(
            &[
                (
                    CellName::testing_new("root"),
                    CellRootPathBuf::testing_new(""),
                ),
                (
                    CellName::testing_new("cell1"),
                    CellRootPathBuf::testing_new("cell1"),
                ),
            ],
            IntentionallyStdHashMap::from([(
                NonEmptyCellAlias::testing_new("alias1"),
                CellName::testing_new("cell1"),
            )]),
        );
        let cell_alias_resolver = cell_resolver.root_cell_cell_alias_resolver();

        let detector = TargetPlatformDetector::parse_spec(
            "target://lib/...->//:p1 target://lib2/foo/...->//:p2 target:alias1//map/...->alias1//:alias",
            CellName::testing_new("root"),
            &cell_resolver,
            cell_alias_resolver,
        )?;

        let p1 = TargetLabel::testing_parse("root//:p1");
        let p2 = TargetLabel::testing_parse("root//:p2");
        let alias = TargetLabel::testing_parse("cell1//:alias");

        assert_eq!(
            detector.detect(&TargetLabel::testing_parse("root//lib/bar:xyz")),
            Some(&p1)
        );
        assert_eq!(
            detector.detect(&TargetLabel::testing_parse("root//lib:xyz")),
            Some(&p1)
        );

        assert_eq!(
            detector.detect(&TargetLabel::testing_parse("root//lib2/foo:xyz")),
            Some(&p2)
        );
        assert_eq!(
            detector.detect(&TargetLabel::testing_parse("root//lib2:foo")),
            None
        );

        assert_eq!(
            detector.detect(&TargetLabel::testing_parse("root//:lib")),
            None
        );

        assert_eq!(
            detector.detect(&TargetLabel::testing_parse("cell1//:xyz")),
            None
        );
        assert_eq!(
            detector.detect(&TargetLabel::testing_parse("cell1//map:xyz")),
            Some(&alias)
        );

        Ok(())
    }
}
