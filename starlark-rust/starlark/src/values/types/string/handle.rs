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

use std::borrow::Borrow;
use std::hash::Hash;
use std::hash::Hasher;

use crate::collections::Hashed;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::layout::static_string::VALUE_EMPTY_STRING;
use crate::values::string::str_type::StarlarkStr;

/// The handle to a Starlark `str`: the [`ValueOf`] of [`StarlarkStr`], with the operations
/// that only strings have.
///
/// One is made by allocating a Rust string (`heap.alloc("magic")`), by [`ValueOf::new`] from a
/// [`Value`] that is a string, or statically by `const_frozen_string!`; it is read with
/// [`as_str`](ValueOf::as_str). It is `Hash`, `Ord` and `Borrow<str>` by its content, so it is a
/// map key that a `&str` looks up.
///
/// ```
/// use starlark::const_frozen_string;
/// use starlark::values::StringValue;
///
/// let s: StringValue<'static> = const_frozen_string!("magic");
/// assert_eq!("magic", s.as_str());
/// ```
pub type StringValue<'v> = ValueOf<'v, StarlarkStr>;

impl<'v> ValueOf<'v, StarlarkStr> {
    /// The Rust string.
    #[inline]
    pub fn as_str(self) -> &'v str {
        self.as_ref().as_str()
    }

    /// Get self along with the hash.
    pub fn get_hashed(self) -> Hashed<Self> {
        Hashed::new_unchecked(self.as_ref().get_hash(), self)
    }

    /// Get the string reference along with the hash.
    pub fn get_hashed_str(self) -> Hashed<&'v str> {
        self.as_ref().as_str_hashed()
    }

    /// Get the [`Value`] along with the hash.
    pub fn get_hashed_value(self) -> Hashed<Value<'v>> {
        Hashed::new_unchecked(self.as_ref().get_hash(), self.to_value())
    }
}

impl<'v> Borrow<str> for ValueOf<'v, StarlarkStr> {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl<'v> Default for ValueOf<'v, StarlarkStr> {
    fn default() -> Self {
        VALUE_EMPTY_STRING.erase().at()
    }
}

impl<'v> Hash for ValueOf<'v, StarlarkStr> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state)
    }
}

impl<'v> PartialOrd for ValueOf<'v, StarlarkStr> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<'v> Ord for ValueOf<'v, StarlarkStr> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::Hashed;
    use crate::const_frozen_string;
    use crate::values::FrozenHeap;
    use crate::values::Heap;
    use crate::values::StringValue;
    use crate::values::Value;
    use crate::values::ValueOf;
    use crate::values::string::str_type::StarlarkStr;

    #[test]
    fn test_string_hashes() {
        let expected = Hashed::new("xyz").hash();

        Heap::temp(|heap| {
            let s: StringValue = heap.alloc("xyz");
            assert_eq!(expected, Hashed::new(s).hash());
            assert_eq!(expected, s.get_hashed().hash());
            let v: Value = heap.alloc("xyz").to_value();
            assert_eq!(expected, v.get_hashed().unwrap().hash());
        });

        FrozenHeap::temp(|heap| {
            let fs: StringValue = heap.alloc("xyz");
            assert_eq!(expected, Hashed::new(fs).hash());
            let fv: Value = heap.alloc("xyz").to_value();
            assert_eq!(expected, fv.get_hashed().unwrap().hash());
        });
    }

    #[test]
    fn test_type_handle() {
        Heap::temp(|heap| {
            let s: ValueOf<StarlarkStr> = heap.alloc("xyz");
            let t: ValueOf<StarlarkStr> = heap.alloc("xyz");
            let u: ValueOf<StarlarkStr> = heap.alloc("xyzw");
            assert!(!s.to_value().ptr_eq(t.to_value()));
            assert_eq!(s, t);
            assert_ne!(s, u);
            assert!(s < u);
            assert_eq!("xyz", s.as_str());
            assert_eq!(Hashed::new("xyz"), s.get_hashed_str());
            assert_eq!(Hashed::new("xyz").hash(), Hashed::new(s).hash());
            assert_eq!(s.get_hashed().hash(), s.get_hashed_value().hash());
            assert_eq!("", ValueOf::<StarlarkStr>::default().as_str());
            let name: ValueOf<'static, StarlarkStr> =
                ValueOf::new(const_frozen_string!("static string").to_value()).unwrap();
            let name: ValueOf<StarlarkStr> = name.at();
            assert_eq!("static string", name.as_str());
        });
    }
}
