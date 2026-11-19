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
use std::cell::Ref;
use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;

use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::package::source_path::SourcePathRef;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use either::Either;

/// A borrowed [`BuildArtifactPath`], which lives either in plain memory or behind a `RefCell`.
pub enum BuildArtifactPathRef<'a> {
    Direct(&'a BuildArtifactPath),
    RefCell(Ref<'a, BuildArtifactPath>),
}

impl Deref for BuildArtifactPathRef<'_> {
    type Target = BuildArtifactPath;

    fn deref(&self) -> &BuildArtifactPath {
        match self {
            Self::Direct(p) => p,
            Self::RefCell(r) => r,
        }
    }
}

impl fmt::Debug for BuildArtifactPathRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl PartialEq for BuildArtifactPathRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for BuildArtifactPathRef<'_> {}

impl Hash for BuildArtifactPathRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

#[derive(Debug, Eq, PartialEq, Hash)]
pub struct ArtifactPath<'a> {
    pub base_path: Either<BuildArtifactPathRef<'a>, SourcePathRef<'a>>,
    pub projected_path: &'a ForwardRelativePath,
    /// The number of components at the prefix of that path that are internal details to the rule,
    /// not returned by `.short_path`.
    pub hidden_components_count: usize,
}

impl ArtifactPath<'_> {
    pub fn with_filename<F, T>(&self, f: F) -> buck2_error::Result<T>
    where
        for<'b> F: FnOnce(&'b FileName) -> T,
    {
        let file_name = match self.projected_path.is_empty() {
            false => self.projected_path,
            true => match self.base_path.as_ref() {
                Either::Left(buck_out) => buck_out.path(),
                Either::Right(buck) => buck.path().as_ref(),
            },
        }
        .file_name()
        .ok_or_else(|| {
            buck2_error!(
                ErrorTag::ArtifactMissingFilename,
                "Artifact has no file name: `{}`",
                self
            )
        })?;

        Ok(f(file_name))
    }

    pub fn with_short_path<F, T>(&self, f: F) -> T
    where
        for<'b> F: FnOnce(&'b ForwardRelativePath) -> T,
    {
        let base_short_path = match self.base_path.as_ref() {
            Either::Left(buck_out) => buck_out.path(),
            Either::Right(buck) => buck.path().as_ref(),
        };

        let path = base_short_path.join_cow(self.projected_path);

        let path = match path.strip_prefix_components(self.hidden_components_count) {
            Some(p) => p,
            None => ForwardRelativePath::empty(),
        };

        f(path)
    }

    pub fn with_full_path<F, T>(&self, f: F) -> T
    where
        for<'b> F: FnOnce(&'b ForwardRelativePath) -> T,
    {
        let base_path = match self.base_path.as_ref() {
            Either::Left(buck_out) => Cow::Borrowed(buck_out.path()),
            Either::Right(buck) => Cow::Owned(
                buck.package()
                    .cell_relative_path()
                    .as_forward_relative_path()
                    .join(buck.path()),
            ),
        };

        let path = base_path.join_cow(self.projected_path);

        f(&path)
    }

    /// Returns the project relative path of the artifact.
    /// A build artifact that is declared to be content-based must have a content hash
    /// provided, otherwise an error is returned.
    pub fn resolve(
        &self,
        artifact_fs: &ArtifactFs,
        content_hash: Option<&ContentBasedPathHash>,
    ) -> buck2_error::Result<ProjectRelativePathBuf> {
        let ArtifactPath {
            base_path,
            projected_path,
            hidden_components_count: _,
        } = self;

        let base_path = match base_path {
            Either::Left(build) => artifact_fs
                .buck_out_path_resolver()
                .resolve_gen(build, content_hash)?,
            Either::Right(source) => artifact_fs.resolve_source(*source)?,
        };

        Ok(base_path.join(projected_path))
    }

    /// This function will return the same project relative path as `resolve_path` except
    /// for content-based artifacts, where it will return a path that uses the configuration
    /// hash instead of the content hash.
    pub fn resolve_configuration_hash_path(
        &self,
        artifact_fs: &ArtifactFs,
    ) -> buck2_error::Result<ProjectRelativePathBuf> {
        let ArtifactPath {
            base_path,
            projected_path,
            hidden_components_count: _,
        } = self;

        let base_path = match base_path {
            Either::Left(build) => artifact_fs
                .buck_out_path_resolver()
                .resolve_gen_configuration_hash_path(build)?,
            Either::Right(source) => artifact_fs.resolve_source(*source)?,
        };

        Ok(base_path.join(projected_path))
    }

    pub fn is_content_based_path(&self) -> bool {
        match self.base_path.as_ref() {
            Either::Left(build_artifact_path) => build_artifact_path.is_content_based_path(),
            Either::Right(_) => false,
        }
    }
}

impl fmt::Display for ArtifactPath<'_> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: This produces a representation we tend to use in Starlark for those, which isn't
        // really consistent with what we use when *not* in Starlark.
        self.with_short_path(|p| write!(fmt, "{p}"))
    }
}
