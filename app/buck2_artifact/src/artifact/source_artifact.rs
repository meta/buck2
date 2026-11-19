/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::package::source_path::SourcePath;
use buck2_core::package::source_path::SourcePathRef;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;

/// An artifact in the source tree
#[derive(
    Clone,
    Debug,
    Display,
    Dupe,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
pub struct SourceArtifact(Arc<SourceArtifactData>);

#[derive(
    Debug,
    Display,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
struct SourceArtifactData(SourcePath);

impl SourceArtifact {
    pub fn new(path: SourcePath) -> Self {
        Self(Arc::new(SourceArtifactData(path)))
    }

    pub fn get_path(&self) -> SourcePathRef<'_> {
        self.0.0.as_ref()
    }
}
