/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::Dupe;

/// Extension trait on [`Result`](Result) where it holds a ref.
pub trait ResultDupedExt {
    type Item;
    type Err;

    /// Like `cloned`, but with a `Dupe` constraint.
    fn duped(self) -> Result<Self::Item, Self::Err>
    where
        Self::Item: Dupe;
}

impl<T, E> ResultDupedExt for Result<&T, E> {
    type Item = T;
    type Err = E;

    fn duped(self) -> Result<T, E>
    where
        T: Dupe,
    {
        self.map(Dupe::dupe)
    }
}

/// Extension trait on [`Result`](Result) where it holds a ref.
pub trait ResultDupedErrExt {
    type Item;
    type Err;

    /// Like `cloned`, but with a `Dupe` constraint.
    fn duped_err(self) -> Result<Self::Item, Self::Err>
    where
        Self::Err: Dupe;
}

impl<T, E> ResultDupedErrExt for Result<T, &E> {
    type Item = T;
    type Err = E;

    fn duped_err(self) -> Result<T, E>
    where
        E: Dupe,
    {
        self.map_err(Dupe::dupe)
    }
}
