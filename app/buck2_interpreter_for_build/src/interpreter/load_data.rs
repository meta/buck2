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
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::legacy_configs::view::LegacyBuckConfigView;
use globset::Glob;
use globset::GlobSet;
use globset::GlobSetBuilder;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;

/// Buckconfig key (`buck2.load_as_allowlist`) holding the allowlist of
/// files that may use the `?as=` import format override, e.g.
/// `load("uv.lock?as=toml", "value")`.
const LOAD_AS_ALLOWLIST: BuckconfigKeyRef = BuckconfigKeyRef {
    section: "buck2",
    property: "load_as_allowlist",
};

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
#[error("Invalid glob in `buck2.load_as_allowlist`: {0}")]
struct InvalidLoadAsGlob(String);

/// Allowlist of files permitted to use an `?as=` import format override
/// (`load("uv.lock?as=toml", "value")`).
///
/// A comma-separated list of globs, each matched against a load target's file
/// name (the last path component). Empty by default, which disables the syntax
/// entirely. Examples:
/// - `*` allows every file.
/// - `*.lock` allows any `.lock` file (including `foo.pkg.lock`).
/// - `*.pkg.lock` allows any `.pkg.lock` file.
/// - `uv.lock` pins a specific package-manager lockfile by its whole name.
#[derive(Debug, Clone, Allocative)]
pub struct LoadAsAllowlist {
    #[allocative(skip)]
    globset: GlobSet,
    /// The globs as written in buckconfig, in the order added to `globset`.
    /// Kept for ser/de (the `GlobSet` is rebuilt from them) and equality.
    patterns: Vec<String>,
}

impl PagableSerialize for LoadAsAllowlist {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.patterns.pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for LoadAsAllowlist {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let patterns = Vec::<String>::pagable_deserialize(deserializer)?;
        let globset = Self::build_globset(&patterns)
            .map_err(|e| pagable::Error::new(e).context("rebuilding LoadAsAllowlist"))?;
        Ok(Self { globset, patterns })
    }
}

impl PartialEq for LoadAsAllowlist {
    fn eq(&self, other: &Self) -> bool {
        // The globset is derived from the patterns, so comparing patterns suffices.
        self.patterns == other.patterns
    }
}

impl Eq for LoadAsAllowlist {}

impl LoadAsAllowlist {
    pub(crate) fn new(values: Vec<String>) -> buck2_error::Result<Self> {
        let patterns: Vec<String> = values
            .into_iter()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .collect();
        let globset =
            Self::build_globset(&patterns).map_err(|e| InvalidLoadAsGlob(e.to_string()))?;
        Ok(Self { globset, patterns })
    }

    pub(crate) fn from_buckconfig(
        mut config: impl LegacyBuckConfigView,
    ) -> buck2_error::Result<Self> {
        Self::new(
            config
                .parse_list::<String>(LOAD_AS_ALLOWLIST)?
                .unwrap_or_default(),
        )
    }

    fn build_globset(patterns: &[String]) -> Result<GlobSet, globset::Error> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            builder.add(Glob::new(pattern)?);
        }
        builder.build()
    }

    /// Whether a file with the given file name (the last path component, e.g.
    /// `uv.lock`) matches any allowlist glob and so may use an `?as=` format
    /// override.
    pub fn is_allowed(&self, file_name: &str) -> bool {
        self.globset.is_match(file_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> LoadAsAllowlist {
        LoadAsAllowlist::new(values.iter().map(|s| (*s).to_owned()).collect()).unwrap()
    }

    #[test]
    fn test_empty_allows_nothing() {
        let allowed = parse(&[]);
        assert!(!allowed.is_allowed("uv.lock"));
        assert!(!allowed.is_allowed("foo.toml"));
    }

    #[test]
    fn test_extension_glob() {
        let allowed = parse(&["*.lock", "*.cfg"]);
        assert!(allowed.is_allowed("uv.lock"));
        assert!(allowed.is_allowed("app.cfg"));
        // `*.lock` also matches a compound extension.
        assert!(allowed.is_allowed("foo.pkg.lock"));
        assert!(!allowed.is_allowed("foo.toml"));
        // A bare `*.lock` glob needs a dot; `mylock` has none.
        assert!(!allowed.is_allowed("mylock"));
    }

    #[test]
    fn test_star_allows_everything() {
        let allowed = parse(&["*"]);
        assert!(allowed.is_allowed("uv.lock"));
        assert!(allowed.is_allowed("foo.anything"));
    }

    #[test]
    fn test_whitespace_and_empty_entries_ignored() {
        let allowed = parse(&["  *.lock  ", ""]);
        assert!(allowed.is_allowed("uv.lock"));
        assert!(!allowed.is_allowed("foo.toml"));
    }

    #[test]
    fn test_compound_extension_glob() {
        let allowed = parse(&["*.pkg.lock"]);
        assert!(allowed.is_allowed("uv.pkg.lock"));
        assert!(allowed.is_allowed("a.b.pkg.lock"));
        assert!(!allowed.is_allowed("uv.lock"));
    }

    #[test]
    fn test_whole_filename_entry() {
        // Pin a specific package-manager lockfile by its whole name.
        let allowed = parse(&["uv.lock"]);
        assert!(allowed.is_allowed("uv.lock"));
        // Other `.lock` files are not opened up.
        assert!(!allowed.is_allowed("poetry.lock"));
        assert!(!allowed.is_allowed("Cargo.lock"));
        // An exact-name glob has no wildcard, so it does not match `nested.uv.lock`.
        assert!(!allowed.is_allowed("nested.uv.lock"));
    }

    #[test]
    fn test_invalid_glob_is_an_error() {
        assert!(LoadAsAllowlist::new(vec!["[".to_owned()]).is_err());
    }
}
