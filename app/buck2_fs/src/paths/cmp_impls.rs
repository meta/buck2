/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! General macros useful for path declaration

use std::cmp;

/// Generates ['cmp::PartialEq'] and ['cmp::PartialOrd'] for the `lhs` and `rhs`
/// types, where `ty` is the unowned, reference path type.
pub macro impl_cmp($lhs:ty, $rhs:ty, $ty:ty) {
    impl cmp::PartialEq<$rhs> for $lhs {
        #[inline]
        fn eq(&self, other: &$rhs) -> bool {
            <$ty as cmp::PartialEq>::eq(self, other)
        }
    }

    impl cmp::PartialEq<$lhs> for $rhs {
        #[inline]
        fn eq(&self, other: &$lhs) -> bool {
            <$ty as cmp::PartialEq>::eq(self, other)
        }
    }

    impl cmp::PartialOrd<$rhs> for $lhs {
        #[inline]
        fn partial_cmp(&self, other: &$rhs) -> Option<cmp::Ordering> {
            <$ty as cmp::PartialOrd>::partial_cmp(self, other)
        }
    }

    impl cmp::PartialOrd<$lhs> for $rhs {
        #[inline]
        fn partial_cmp(&self, other: &$lhs) -> Option<cmp::Ordering> {
            <$ty as cmp::PartialOrd>::partial_cmp(self, other)
        }
    }
}

use crate::paths::forward_rel_path::ForwardRelativePath;
use crate::paths::forward_rel_path::ForwardRelativePathBuf;
use crate::paths::relative_path::RelativePath;
use crate::paths::relative_path::RelativePathBuf;

impl_cmp!(
    ForwardRelativePathBuf,
    ForwardRelativePath,
    ForwardRelativePath
);
impl_cmp!(
    ForwardRelativePathBuf,
    &'_ ForwardRelativePath,
    ForwardRelativePath
);

impl_cmp!(RelativePathBuf, RelativePath, RelativePath);
impl_cmp!(RelativePathBuf, &'_ RelativePath, RelativePath);
