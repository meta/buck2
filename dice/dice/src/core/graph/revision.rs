/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The identities of `dice_core`, as dice names them, and the mint of the revisions dice hands
//! the core.

use allocative::Allocative;
pub(crate) use dice_core::EpsilonToken;
pub(crate) use dice_core::Revision;

/// A mint of fresh revisions for one key, starting at [`Revision::FIRST`]. Aborts rather than
/// wrapping on overflow.
#[derive(Debug, Allocative)]
pub(crate) struct RevisionMint(Revision);

impl RevisionMint {
    pub(crate) fn new() -> Self {
        Self(Revision::FIRST)
    }

    pub(crate) fn mint(&mut self) -> Revision {
        let r = self.0;
        self.0 = r.next();
        r
    }
}

impl Default for RevisionMint {
    fn default() -> Self {
        Self::new()
    }
}
