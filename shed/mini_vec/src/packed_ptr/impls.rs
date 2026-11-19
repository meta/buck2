/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;

use allocative::Allocative;
use dupe::Dupe;

use crate::packed_ptr::def::PackedPtr;
use crate::packed_ptr::ptr_value::PointerValue;

impl<P: PointerValue> fmt::Debug for PackedPtr<P>
where
    for<'a> P::Ref<'a>: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PackedPtr")
            .field("ptr", &self.as_ref())
            .field("extra", &self.extra())
            .finish()
    }
}

impl<P: PointerValue> PartialEq for PackedPtr<P>
where
    for<'a> P::Ref<'a>: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.extra() == other.extra() && self.as_ref() == other.as_ref()
    }
}

impl<P: PointerValue> Eq for PackedPtr<P> where for<'a> P::Ref<'a>: Eq {}

impl<P: PointerValue> Hash for PackedPtr<P>
where
    for<'a> P::Ref<'a>: Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.extra().hash(state);
        self.as_ref().hash(state);
    }
}

impl<P: PointerValue + Dupe> Dupe for PackedPtr<P> where P::Storage: Clone {}

impl<P: PointerValue + Allocative> Allocative for PackedPtr<P> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        // Account for the `PackedPtr` word itself (the pointer plus the packed `extra` byte, or both
        // fields in the unpacked representation — either way exactly `size_of::<Self>()`), then
        // delegate to `P`'s own `Allocative` impl for whatever it owns beyond that. In the packed
        // build the nested `P` self node is the same size as this word, so it folds into this node
        // rather than being double-counted; in the unpacked build the extra byte's word remains and
        // is correctly attributed here.
        let mut visitor = visitor.enter_self_sized::<Self>();
        self.with_inner(|p| p.visit(&mut visitor));
        visitor.exit();
    }
}
