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
pub(crate) const LOAD_AS_ALLOWLIST: BuckconfigKeyRef = BuckconfigKeyRef {
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

    fn build_globset(patterns: &[String]) -> Result<GlobSet, globset::Error> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            builder.add(Glob::new(pattern)?);
        }
        builder.build()
    }

    /// Whether a file with the given file name (the last path component, e.g.
    /// `uv.lock`, or `None` when there is no file name) matches any allowlist
    /// glob and so may use an `?as=` format override.
    pub fn is_allowed(&self, file_name: Option<&str>) -> bool {
        match file_name {
            Some(file_name) => self.globset.is_match(file_name),
            None => false,
        }
    }
}
