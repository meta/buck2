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
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

/// The `oncall` annotation for a `BUCK` file.
#[derive(
    Debug, Hash, StrongHash, Allocative, Eq, PartialEq, Dupe, Clone, Pagable
)]
pub struct Oncall(Arc<String>);

impl Oncall {
    pub fn new(oncall: &str) -> Self {
        Self(Arc::new(oncall.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
