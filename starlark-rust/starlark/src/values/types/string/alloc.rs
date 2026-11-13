/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! The ways to allocate a string other than copying one Rust string, which is what allocating
//! a `&str` or a `String` does.

use crate::values::AllocValue;
use crate::values::Heap;
use crate::values::ValueOf;
use crate::values::string::str_type::StarlarkStr;

/// Allocates a string on a [`Heap`], sharing the allocation with any equal string interned on
/// that heap before.
///
/// Prefer this for strings that are allocated repeatedly and kept alive; allocating a `&str`
/// copies it every time. A [`FrozenHeap`](crate::values::FrozenHeap) always interns, so there
/// a `&str` is enough.
///
/// ```
/// use starlark::values::Heap;
/// use starlark::values::string::AllocStrIntern;
///
/// Heap::temp(|heap| {
///     let first = heap.alloc(AllocStrIntern("interned"));
///     let second = heap.alloc(AllocStrIntern("interned"));
///     assert!(first.to_value().ptr_eq(second.to_value()));
/// });
/// ```
pub struct AllocStrIntern<'a>(pub &'a str);

impl<'v> AllocValue<'v> for AllocStrIntern<'_> {
    type Type = StarlarkStr;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        heap.alloc_str_intern(self.0)
    }
}

/// Allocates the concatenation of two strings on a [`Heap`], without building it in Rust first.
///
/// ```
/// use starlark::values::Heap;
/// use starlark::values::string::AllocStrConcat;
///
/// Heap::temp(|heap| {
///     assert_eq!("foo.rs", heap.alloc(AllocStrConcat("foo", ".rs")).as_str());
/// });
/// ```
pub struct AllocStrConcat<'a>(pub &'a str, pub &'a str);

impl<'v> AllocValue<'v> for AllocStrConcat<'_> {
    type Type = StarlarkStr;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        heap.alloc_str_concat(self.0, self.1)
    }
}

#[cfg(test)]
mod tests {
    use crate::values::Heap;
    use crate::values::string::AllocStrConcat;
    use crate::values::string::AllocStrIntern;

    #[test]
    fn test_intern() {
        Heap::temp(|heap| {
            let first = heap.alloc(AllocStrIntern("interned"));
            let second = heap.alloc(AllocStrIntern("interned"));
            assert!(first.to_value().ptr_eq(second.to_value()));
            let copy = heap.alloc("interned");
            assert!(!first.to_value().ptr_eq(copy.to_value()));
            assert_eq!(first, copy);
        });
    }

    #[test]
    fn test_concat() {
        Heap::temp(|heap| {
            assert_eq!("foo.rs", heap.alloc(AllocStrConcat("foo", ".rs")).as_str());
            assert_eq!("foo", heap.alloc(AllocStrConcat("foo", "")).as_str());
            assert_eq!(".rs", heap.alloc(AllocStrConcat("", ".rs")).as_str());
        });
    }
}
