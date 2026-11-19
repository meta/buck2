/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Debug;

use buck2_common::package_listing::file_listing::PackageFileListing;
use buck2_core::package::package_relative_path::PackageRelativePath;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckMutSet;
use derivative::Derivative;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum GlobError {
    #[error(
        "Exact pattern must be valid forward relative path: `{0}` \
        (this pattern cannot match anything)"
    )]
    ExactIncludeMustBeForwardRelativePath(String),
    #[error("Double slash in pattern: `{0}` (this pattern cannot match anything)")]
    DoubleSlash(String),
    #[error("Pattern must not start with `/`: `{0}` (this pattern cannot match anything)")]
    LeadingSlash(String),
    #[error("Pattern must not end with `/`: `{0}` (this pattern cannot match anything)")]
    TrailingSlash(String),
    #[error("Pattern must not contain `..`: `{0}` (this pattern cannot match anything)")]
    DotDot(String),
    #[error("Pattern must not contain `.`: `{0}` (this pattern cannot match anything)")]
    Dot(String),
}

/// The default Debug for Pattern is horribly verbose with lots of internal
/// details we don't care about, so create `GlobPattern` so we can have a
/// readable `Debug`.
struct GlobPattern(glob::Pattern);

impl Debug for GlobPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Pattern").field(&self.0.as_str()).finish()
    }
}

impl GlobPattern {
    fn new(pattern: &str) -> buck2_error::Result<GlobPattern> {
        let parsed_pattern = glob::Pattern::new(pattern)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Input))
            .with_buck_error_context(|| format!("Error creating globspec for `{pattern}`"))?;
        if pattern.contains("//") {
            return Err(GlobError::DoubleSlash(pattern.to_owned()).into());
        }
        if pattern.starts_with('/') {
            return Err(GlobError::LeadingSlash(pattern.to_owned()).into());
        }
        if pattern.ends_with('/') {
            return Err(GlobError::TrailingSlash(pattern.to_owned()).into());
        }
        // TODO(nga): also ban single dot.
        if pattern.split('/').any(|p| p == "..") {
            return Err(GlobError::DotDot(pattern.to_owned()).into());
        }
        if pattern.split('/').any(|p| p == ".") {
            return Err(GlobError::Dot(pattern.to_owned()).into());
        }
        Ok(GlobPattern(parsed_pattern))
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
pub(crate) struct GlobSpec {
    /// Byte prefixes that cover every file the patterns can match; empty means no narrowing.
    narrowing_prefixes: Vec<String>,
    exact_matches: BuckMutSet<String>,
    patterns: Vec<GlobPattern>,
    excludes: Vec<GlobPattern>,
}

/// Returns the no-special-characters common prefix of all patterns. Suitable for binary search.
fn longest_common_glob_prefix<P: AsRef<str>>(patterns: &[P]) -> &str {
    if patterns.is_empty() {
        return "";
    }
    let first: &str = patterns[0].as_ref();
    // Globs have some special characters, ensure they're not present. `len` counts chars.
    let mut len = first
        .chars()
        .position(|c| "*[]?".contains(c))
        .unwrap_or_else(|| first.chars().count());
    for pattern in patterns {
        if len == 0 {
            break;
        }
        len = std::cmp::min(
            len,
            pattern
                .as_ref()
                .chars()
                .take(len)
                .zip(first.chars())
                .take_while(|&(a, b)| a == b)
                .count(),
        );
    }
    match first.char_indices().nth(len) {
        None => first,
        Some((idx, _)) => &first[..idx],
    }
}

/// Every ASCII-case variant of the common prefix, since `matches` compares ASCII letters
/// case-insensitively but `files_with_prefix` compares bytes. The prefix is cut after
/// `MAX_CASED_LETTERS` letters to bound the number of variants; a shorter prefix only narrows less.
fn narrowing_prefixes(common_prefix: &str) -> Vec<String> {
    const MAX_CASED_LETTERS: usize = 4;
    let mut variants = vec![String::new()];
    let mut letters = 0;
    for c in common_prefix.chars() {
        if c.is_ascii_alphabetic() {
            if letters == MAX_CASED_LETTERS {
                break;
            }
            letters += 1;
            variants = variants
                .into_iter()
                .flat_map(|variant| {
                    let mut lower = variant.clone();
                    lower.push(c.to_ascii_lowercase());
                    let mut upper = variant;
                    upper.push(c.to_ascii_uppercase());
                    [lower, upper]
                })
                .collect();
        } else {
            for variant in &mut variants {
                variant.push(c);
            }
        }
    }
    variants.retain(|variant| !variant.is_empty());
    variants
}

impl GlobSpec {
    const BINARY_SEARCH_CUTOFF: usize = 100;
    pub(crate) fn new<P: AsRef<str>, Q: AsRef<str>>(
        patterns: &[P],
        excludes: &[Q],
    ) -> buck2_error::Result<Self> {
        let mut glob_patterns = Vec::new();
        let mut glob_excludes = Vec::new();
        let mut exact_matches = BuckMutSet::default();
        for pattern in patterns {
            let pattern = pattern.as_ref();
            if pattern.contains('*') {
                glob_patterns.push(GlobPattern::new(pattern)?);
            } else {
                // TODO(nga): pattern `*/[bc]` is parsed as glob pattern,
                //   but `a/[bc]` is parsed as exact match?
                //   Does not look right.

                if ForwardRelativePath::new(&pattern).is_err() {
                    return Err(GlobError::ExactIncludeMustBeForwardRelativePath(
                        pattern.to_owned(),
                    )
                    .into());
                }
                exact_matches.insert(pattern.to_owned());
            }
        }
        for pattern in excludes {
            let pattern = pattern.as_ref();
            glob_excludes.push(GlobPattern::new(pattern)?);
        }
        Ok(Self {
            narrowing_prefixes: narrowing_prefixes(longest_common_glob_prefix(patterns)),
            exact_matches,
            patterns: glob_patterns,
            excludes: glob_excludes,
        })
    }

    pub(crate) fn matches(&self, path: &str) -> bool {
        let options = glob::MatchOptions {
            require_literal_separator: true,
            require_literal_leading_dot: true,
            // FIXME: We should have case sensitive globs
            case_sensitive: false,
        };
        let include_matches = self.exact_matches.contains(path)
            || self
                .patterns
                .iter()
                .any(|p| p.0.matches_with(path, options));
        include_matches
            && !self
                .excludes
                .iter()
                .any(|p| p.0.matches_with(path, options))
    }

    pub(crate) fn resolve_glob<'a>(
        &'a self,
        spec: &'a PackageFileListing,
    ) -> Box<dyn Iterator<Item = &'a PackageRelativePath> + 'a> {
        if spec.files().len() >= Self::BINARY_SEARCH_CUTOFF && !self.narrowing_prefixes.is_empty() {
            let mut candidates: Vec<_> = self
                .narrowing_prefixes
                .iter()
                .flat_map(|prefix| spec.files_with_prefix(prefix))
                .collect();
            // The variants are distinct byte prefixes of one length, so their ranges are
            // disjoint; sorting restores the listing order.
            candidates.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
            return Box::new(
                candidates
                    .into_iter()
                    .filter(move |v| self.matches(v.as_str())),
            );
        }
        Box::new(spec.files().filter(move |v| self.matches(v.as_str())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() -> buck2_error::Result<()> {
        let spec = GlobSpec::new(
            &[
                "abc*",
                "**/*.java",
                "*/*/*.txt",
                "unglobbed",
                "excluded/unglobbed",
            ],
            &["excluded/**/*"],
        )?;

        assert!(spec.matches("abcxyz"));
        assert!(spec.matches("abcsomething"));
        assert!(spec.matches("1/2/3.txt"));
        assert!(spec.matches("dir/other/more/some.java"));
        assert!(spec.matches("unglobbed"));

        assert!(!spec.matches("xyz"));
        assert!(!spec.matches("abc/something"));
        assert!(!spec.matches("1/2.txt"));
        assert!(!spec.matches("1/2/3/4.txt"));

        assert!(!spec.matches("excluded/subdir/some.java"));
        assert!(!spec.matches("excluded/some.java"));
        assert!(!spec.matches("excluded/unglobbed"));

        Ok(())
    }

    #[test]
    fn test_glob_match_case_insensitive() -> buck2_error::Result<()> {
        // NOTE: We probably should change this. But for now, let's codify the current behavior
        // since that's probably something that should require a migration.

        let spec = GlobSpec::new(&["src/**/*"], &["src/excluded/**/*"])?;
        assert!(spec.matches("src/foo"));
        assert!(spec.matches("SRC/foo"));
        assert!(!spec.matches("src/EXCLUDED/bar"));

        Ok(())
    }

    #[test]
    fn test_resolve_glob() -> buck2_error::Result<()> {
        let spec = GlobSpec::new(&["abc*", "**/*.java", "*/*/*.txt"], &["excluded/**/*"])?;

        let package_listing = PackageFileListing::testing_new(&[
            "abcxyz",
            "abcsomething",
            "1/2/3.txt",
            "dir/other/more/some.java",
            "xyz",
            "abc/something",
            "1/2.txt",
            "1/2/3/4.txt",
            "excluded/subdir/some.java",
            "excluded/some.java",
        ]);

        let matches: Vec<_> = spec
            .resolve_glob(&package_listing)
            .map(|p| p.as_str())
            .collect();

        assert_eq!(
            vec![
                "1/2/3.txt",
                "abcsomething",
                "abcxyz",
                "dir/other/more/some.java",
            ],
            matches
        );

        Ok(())
    }

    #[test]
    fn test_resolve_glob_dot() {
        fn glob(pattern: &str, listing: &PackageFileListing) -> Vec<String> {
            GlobSpec::new(&[pattern], &[""; 0])
                .unwrap()
                .resolve_glob(listing)
                .map(|p| p.as_str().to_owned())
                .collect()
        }

        let listing = PackageFileListing::testing_new(&["a", ".a", "b/c", "b/.c", ".d/e", ".d/.e"]);

        // Simple difference between dot and non-dot patterns.
        assert_eq!(vec![".a"], glob(".*", &listing));
        assert_eq!(vec!["a"], glob("*", &listing));

        // Match works in subdirectories too, even if directory name starts with dot.
        assert_eq!(vec![".d/e"], glob(".d/*", &listing));
        assert_eq!(vec![".d/.e"], glob(".d/.*", &listing));

        // Star matches directory, but not dot-directory.
        assert_eq!(vec!["b/.c"], glob("*/.*", &listing));
        assert_eq!(vec!["b/c"], glob("*/*", &listing));

        // Dot-star matches dot-directory, but not directory.
        assert_eq!(vec![".d/.e"], glob(".*/.*", &listing));
        assert_eq!(vec![".d/e"], glob(".*/*", &listing));

        // Star-star matches only non-star directories and files.
        assert_eq!(vec!["a", "b/c"], glob("**", &listing));
        assert_eq!(vec![".a", "b/.c"], glob("**/.*", &listing));
    }

    #[test]
    fn test_resolve_glob_square_brackets_bug() {
        let glob = GlobSpec::new(&["*/[bc]"], &[""; 0]).unwrap();
        assert!(glob.matches("a/b"));
        let glob = GlobSpec::new(&["a/[bc]"], &[""; 0]).unwrap();
        // TODO(nga): This is a bug.
        assert!(!glob.matches("a/b"));
    }

    #[test]
    fn test_invalid_patterns() {
        fn test_invalid(pattern: &str) {
            assert!(
                GlobSpec::new(&[pattern], &[""; 0]).is_err(),
                "Pattern must not be accepted: `{pattern}`"
            );
            assert!(
                GlobSpec::new(&[""; 0], &[pattern]).is_err(),
                "Pattern must not be accepted as exclude as well: `{pattern}`"
            );
        }

        test_invalid("a**");
        test_invalid("**a");
        test_invalid(".**");
    }

    #[test]
    fn test_leading_slash_in_pattern() {
        assert!(GlobSpec::new(&["/*"], &[""; 0]).is_err());
        assert!(GlobSpec::new(&["/a"], &[""; 0]).is_err());
    }

    #[test]
    fn test_dot_dot_in_pattern() {
        assert!(GlobSpec::new(&[".."], &[""; 0]).is_err());
        assert!(GlobSpec::new(&["../*"], &[""; 0]).is_err());
        assert!(GlobSpec::new(&["a/../*"], &[""; 0]).is_err());
        assert!(GlobSpec::new(&["*/../b"], &[""; 0]).is_err());
    }

    #[test]
    fn test_dot_in_pattern() {
        assert!(GlobSpec::new(&["."], &[""; 0]).is_err());
        assert!(GlobSpec::new(&["./a"], &[""; 0]).is_err());
        assert!(GlobSpec::new(&["a/./b"], &[""; 0]).is_err());
        assert!(GlobSpec::new(&["a/."], &[""; 0]).is_err());
    }

    #[test]
    fn test_double_slash_in_pattern() {
        assert!(GlobSpec::new(&["//"], &[""; 0]).is_err());
        assert!(GlobSpec::new(&["a//**"], &[""; 0]).is_err());
        assert!(GlobSpec::new(&["a/*//b"], &[""; 0]).is_err());
    }

    #[test]
    fn test_trailing_slash_in_pattern() {
        assert!(GlobSpec::new(&["a/"], &[""; 0]).is_err());
        assert!(GlobSpec::new(&["a/*/"], &[""; 0]).is_err());
    }

    fn resolve_for_test(patterns: &[&str], files: &[&str]) -> Vec<String> {
        let spec = GlobSpec::new(patterns, &[""; 0]).unwrap();
        let listing = PackageFileListing::testing_new(files);
        spec.resolve_glob(&listing)
            .map(|p| p.as_str().to_owned())
            .collect()
    }

    /// `file` plus enough unrelated files to reach `BINARY_SEARCH_CUTOFF`.
    fn large_listing(file: &str) -> Vec<String> {
        let mut files: Vec<String> = (0..GlobSpec::BINARY_SEARCH_CUTOFF - 1)
            .map(|i| format!("z{i:03}"))
            .collect();
        files.push(file.to_owned());
        files
    }

    #[test]
    fn test_resolve_glob_non_ascii_prefix() {
        assert_eq!("\u{e9}", longest_common_glob_prefix(&["\u{e9}*"]));
        assert_eq!(
            "donn\u{e9}es/",
            longest_common_glob_prefix(&["donn\u{e9}es/*.h", "donn\u{e9}es/*.c"])
        );

        // The result does not depend on the size of the package.
        assert_eq!(vec!["\u{e9}"], resolve_for_test(&["\u{e9}*"], &["\u{e9}"]));
        let files = large_listing("\u{e9}");
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        assert_eq!(vec!["\u{e9}"], resolve_for_test(&["\u{e9}*"], &files));

        let files = large_listing("donn\u{e9}es/a.txt");
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        assert_eq!(
            vec!["donn\u{e9}es/a.txt"],
            resolve_for_test(&["donn\u{e9}es/*"], &files)
        );
    }

    #[test]
    fn test_resolve_glob_large_listing_letter_case() {
        // Patterns match case-insensitively whatever the size of the package.
        assert_eq!(vec!["a"], resolve_for_test(&["A*"], &["a"]));
        let files = large_listing("a");
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        assert_eq!(vec!["a"], resolve_for_test(&["A*"], &files));

        assert_eq!(
            vec!["SRC/foo"],
            resolve_for_test(&["src/**/*"], &["SRC/foo"])
        );
        let files = large_listing("SRC/foo");
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        assert_eq!(vec!["SRC/foo"], resolve_for_test(&["src/**/*"], &files));

        // Results keep the listing order across the narrowed ranges.
        let mut files = large_listing("abc/1");
        files.push("ABC/2".to_owned());
        files.push("aBc/3".to_owned());
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        assert_eq!(
            vec!["ABC/2", "aBc/3", "abc/1"],
            resolve_for_test(&["abc/*"], &files)
        );
    }

    #[test]
    fn test_narrowing_prefixes() {
        assert_eq!(Vec::<String>::new(), narrowing_prefixes(""));
        assert_eq!(vec!["1/"], narrowing_prefixes("1/"));
        assert_eq!(vec!["s/", "S/"], narrowing_prefixes("s/"));
        assert_eq!(vec!["sr", "sR", "Sr", "SR"], narrowing_prefixes("sr"));
        // Cut after four letters: 16 variants of `abcd`, and `e/` is dropped.
        let prefixes = narrowing_prefixes("abcde/");
        assert_eq!(16, prefixes.len());
        assert!(prefixes.iter().all(|p| p.eq_ignore_ascii_case("abcd")));
    }

    #[test]
    fn test_glob_prefix() {
        assert_eq!("a/", longest_common_glob_prefix(&["a/1", "a/2"]));
        assert_eq!("", longest_common_glob_prefix(&["a/1", "b/1"]));
        assert_eq!("a/", longest_common_glob_prefix(&["a/**/*.h", "a/**/*.c"]));
        assert_eq!("a/", longest_common_glob_prefix(&["a/in/*.h", "a/**/*.c"]));
        // '?' is a single-character wildcard.
        assert_eq!("a/", longest_common_glob_prefix(&["a/foo", "a/?"]));
        // '[' and ']' are special character set tokens.
        assert_eq!("f/", longest_common_glob_prefix(&["f/[ab]/*", "f/[abc]/*"]));
        // These codepoints have a common prefix in bytes, but not in chars.
        assert_eq!("", longest_common_glob_prefix(&["\u{0306}", "\u{0307}"]));
    }
}
