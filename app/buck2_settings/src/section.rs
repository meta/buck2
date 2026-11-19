/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ops::Deref;
use std::sync::Arc;

use allocative::Allocative;
use dupe::Dupe;
use serde::Deserialize;
use serde::Serialize;

/// A section as `BuckSettings` holds it: shared, so that handing the settings
/// to every command copies pointers, and read through the section type's
/// accessors.
#[derive(Debug, Serialize, Deserialize, Allocative)]
#[serde(transparent)]
pub struct Section<T>(Arc<T>);

impl<T> Section<T> {
    pub fn new(section: T) -> Self {
        Self(Arc::new(section))
    }
}

impl<T> Clone for Section<T> {
    fn clone(&self) -> Self {
        Self(self.0.dupe())
    }
}

impl<T> Dupe for Section<T> {}

impl<T: Default> Default for Section<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: PartialEq> PartialEq for Section<T> {
    fn eq(&self, other: &Self) -> bool {
        *self.0 == *other.0
    }
}

impl<T: Eq> Eq for Section<T> {}

impl<T> Deref for Section<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> From<T> for Section<T> {
    fn from(section: T) -> Self {
        Self::new(section)
    }
}
