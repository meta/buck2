/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(feature = "jiff")]

use jiff::SignedDuration;
use jiff::Timestamp;

use crate::allocative_trait::Allocative;
use crate::visitor::Visitor;

impl Allocative for Timestamp {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        visitor.enter_self_sized::<Self>().exit();
    }
}

impl Allocative for SignedDuration {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        visitor.enter_self_sized::<Self>().exit();
    }
}

#[cfg(test)]
mod tests {
    use jiff::SignedDuration;
    use jiff::Timestamp;

    use crate::golden::golden_test;

    #[test]
    fn test_timestamp() {
        golden_test!(&Timestamp::UNIX_EPOCH);
    }

    #[test]
    fn test_signed_duration() {
        golden_test!(&SignedDuration::ZERO);
    }
}
