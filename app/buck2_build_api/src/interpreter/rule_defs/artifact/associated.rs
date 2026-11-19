/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use buck2_hash::BuckFixedInsertionSet;
use dupe::*;
use pagable::Pagable;
use starlark::values::StarlarkPagableViaPagable;
use starlark::values::Trace;

use crate::artifact_groups::ArtifactGroup;

#[derive(
    Debug,
    Clone,
    Dupe_,
    Allocative,
    Trace,
    PartialEq,
    Pagable,
    StarlarkPagableViaPagable
)]
pub struct AssociatedArtifacts(Option<Arc<BuckFixedInsertionSet<ArtifactGroup>>>);

impl AssociatedArtifacts {
    pub fn new() -> Self {
        Self(None)
    }

    pub fn union(&self, other: AssociatedArtifacts) -> AssociatedArtifacts {
        match (&self.0, &other.0) {
            (_, None) => self.dupe(),
            (None, _) => other,
            (Some(left), Some(right)) => Self::from(left.iter().chain(right.iter()).duped()),
        }
    }

    pub fn from<T: IntoIterator<Item = ArtifactGroup>>(from: T) -> Self {
        let values: BuckFixedInsertionSet<_> = from.into_iter().collect();
        if values.is_empty() {
            Self(None)
        } else {
            Self(Some(Arc::new(values)))
        }
    }

    pub fn len(&self) -> usize {
        match &self.0 {
            Some(v) => v.len(),
            None => 0,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ArtifactGroup> {
        self.0.iter().flat_map(|v| v.iter())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
