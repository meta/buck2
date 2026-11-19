/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::sync::Arc;

use allocative::Allocative;
use buck2_build_api::artifact_groups::promise::PromiseArtifact;
use buck2_build_api::artifact_groups::promise::PromiseArtifactId;
use buck2_build_api::artifact_groups::promise::PromiseArtifactLock;
use dupe::Dupe;
use starlark::codemap::FileSpan;
use starlark::values::Trace;

#[derive(Debug, Trace, Allocative)]
struct PromiseArtifactEntry {
    location: Option<FileSpan>,
    artifact: PromiseArtifact,
}

/// The PromiseArtifactRegistry stores promises registered with `artifact_promise_mappings` in `anon_rule()`, and their
/// corresponding internal PromiseArtifact. At the end of analysis (after promises have been resolved),
/// all PromiseArtifact will be updated to have the resolved artifact from the corresponding starlark promise.
#[derive(Debug, Trace, Allocative)]
pub(crate) struct PromiseArtifactRegistry {
    artifacts: Vec<PromiseArtifactEntry>,
}

impl PromiseArtifactRegistry {
    pub(crate) fn new() -> Self {
        Self {
            artifacts: Vec::new(),
        }
    }

    /// The consumer analysis is the analysis that calls the anon target and uses the resulting
    /// promised artifacts. It could be a normal rule analysis, an analysis from BXL, or an anon
    /// target analysis. These promised artifacts are the ones that will have their short paths
    /// asserted. During promise resolution, we use the promised artifact's owner (the anon target
    /// key) to look up the owner's analysis results via DICE (which will be blocking) to ensure
    /// that any dependent anon target analyses are finished first.
    pub(crate) fn consumer_analysis_artifacts(&self) -> Vec<PromiseArtifact> {
        self.artifacts
            .iter()
            .map(|e| e.artifact.clone())
            .collect::<Vec<_>>()
    }

    pub(crate) fn register(
        &mut self,
        location: Option<FileSpan>,
        id: PromiseArtifactId,
    ) -> buck2_error::Result<PromiseArtifact> {
        let artifact: PromiseArtifact =
            PromiseArtifact::new(Arc::new(PromiseArtifactLock::new()), id);

        self.artifacts.push(PromiseArtifactEntry {
            location,
            artifact: artifact.dupe(),
        });
        Ok(artifact)
    }
}
