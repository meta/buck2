/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;

pub trait ArtifactDyn: Send + Sync + 'static {
    /// Returns the project relative path of the artifact.
    /// A build artifact that is declared to be content-based must have a content hash
    /// provided, otherwise an error is returned.
    fn resolve_path(
        &self,
        fs: &ArtifactFs,
        content_hash: Option<&ContentBasedPathHash>,
    ) -> buck2_error::Result<ProjectRelativePathBuf>;

    /// This function will return the same project relative path as `resolve_path` except
    /// for content-based artifacts, where it will return a path that uses the configuration
    /// hash instead of the content hash.
    fn resolve_configuration_hash_path(
        &self,
        fs: &ArtifactFs,
    ) -> buck2_error::Result<ProjectRelativePathBuf>;
    /// Build artifacts and source artifacts from external cells require materialization. Other
    /// source artifacts do not.
    fn requires_materialization(&self, fs: &ArtifactFs) -> bool;

    fn has_content_based_path(&self) -> bool;

    fn is_projected(&self) -> bool;
}
