/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;

use crate::artifact_value::ArtifactValue;
use crate::directory::LazyActionDirectoryBuilder;

/// This is like `ArtifactGroupValues`, but without dependency on `Artifact`.
pub trait ArtifactGroupValuesDyn: Send + Sync + 'static {
    fn iter(&self) -> Box<dyn Iterator<Item = (&dyn ArtifactDyn, &ArtifactValue)> + '_>;

    fn add_to_directory(
        &self,
        builder: &mut LazyActionDirectoryBuilder,
        artifact_fs: &ArtifactFs,
    ) -> buck2_error::Result<()>;
}
