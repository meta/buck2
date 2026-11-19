/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::fmt;
use std::fmt::Display;

use allocative::Allocative;
use dupe::Dupe;

#[allow(unused)]
#[derive(Eq, PartialEq, Copy, Clone, Debug, Hash, Allocative)]
pub struct NoHash(Infallible);

impl Display for NoHash {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {}
    }
}

impl Dupe for NoHash {}
