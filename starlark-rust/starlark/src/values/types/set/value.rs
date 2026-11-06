/*
 * Copyright 2018 The Starlark in Rust Authors.
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

use std::cell::Ref;
use std::cell::RefCell;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::mem;
use std::ops::Deref;

use allocative::Allocative;
use display_container::fmt_container;
use either::Either;
use serde::Serialize;
use starlark_map::Hashed;
use starlark_map::small_set::SmallSet;

use super::refs::SetRef;
use crate as starlark;
use crate::environment::Methods;
use crate::typing::Ty;
use crate::util::refcell::unleak_borrow;
use crate::values::AllocValue;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::Heap;
use crate::values::ProvidesStaticType;
use crate::values::StarlarkPagable;
use crate::values::StarlarkRepresentations;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::Trace;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::ValueTyped;
use crate::values::comparison::equals_small_set;
use crate::values::set::methods;
use crate::values::starlark_value;
use crate::values::type_repr::SetType;
use crate::values::type_repr::StarlarkTypeRepr;

#[derive(
    Clone,
    Default,
    Trace,
    Debug,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(transparent)]
pub(crate) struct SetGen<T>(pub(crate) T);

/// Define the set type.
#[derive(
    Default,
    Trace,
    Debug,
    ProvidesStaticType,
    Allocative,
    Clone,
    StarlarkPagable
)]
pub(crate) struct SetData<'v> {
    /// The data stored by the list.
    pub(crate) content: SmallSet<Value<'v>>,
}

impl<'v> SetData<'v> {
    pub fn clear(&mut self) {
        self.content.clear();
    }

    /// Iterate through the values in the set.
    pub fn iter<'a>(&'a self) -> impl ExactSizeIterator<Item = Value<'v>> + 'a {
        self.content.iter().copied()
    }

    /// Iterate through the values in the set, but retaining the hash of the values.
    pub fn iter_hashed<'a>(&'a self) -> impl Iterator<Item = Hashed<Value<'v>>> + 'a
    where
        'v: 'a,
    {
        self.content.iter_hashed().map(|h| h.copied())
    }

    /// Check if the set contains an hashed element.
    pub(crate) fn contains_hashed(&self, key: Hashed<Value<'v>>) -> bool {
        self.content.contains_hashed(key.as_ref())
    }

    pub(crate) fn add_hashed(&mut self, value: Hashed<Value<'v>>) -> bool {
        self.content.insert_hashed(value)
    }

    pub fn remove_hashed(&mut self, value: Hashed<&Value<'v>>) -> bool {
        self.content.shift_remove_hashed(value)
    }
}

pub(crate) type MutableSet<'v> = SetGen<RefCell<SetData<'v>>>;

/// A frozen set is a `SetData` without the `RefCell`: the two variants differ only in whether
/// mutation is possible, and nothing ever hands out a `&mut` to the frozen one.
pub(crate) type FrozenSet = SetGen<SetData<'static>>;

crate::register_simple_vtable_entry!(FrozenSet);
// SAFETY: The vtable entry is registered above. The deser type id is
// lifetime-erased, so the `'static` instantiation covers all heap lifetimes.
unsafe impl<'v> crate::__derive_refs::VtableRegistered for SetGen<SetData<'v>> {}

impl<'v> AllocValue<'v> for SetData<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(SetGen(RefCell::new(self)))
    }
}

impl<'v> StarlarkTypeRepr for SetData<'v> {
    type Canonical = <SetType<Value<'static>> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        SetType::<Value<'v>>::starlark_type_repr()
    }
}

// TODO Add optimizations not to allocate empty set.
impl<'v> Freeze<'v> for MutableSet<'v> {
    type Frozen<'fv> = SetGen<SetData<'fv>>;
    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let values = self.0.into_inner().content;
        let mut content = SmallSet::with_capacity(values.len());
        for value in values.into_iter_hashed() {
            // Freezing does not change the hash.
            let value = Hashed::new_unchecked(value.hash(), freezer.freeze(value.into_key())?);
            content.insert_hashed_unique_unchecked(value);
        }
        Ok(SetGen(SetData { content }))
    }
}

trait SetLike<'v>: Debug + Allocative {
    type ContentRef<'a>: Deref<Target = SmallSet<Value<'v>>>
    where
        Self: 'a,
        'v: 'a;
    fn content<'a>(&'a self) -> Self::ContentRef<'a>;

    // These functions are unsafe for the same reason
    // `StarlarkValue` iterator functions are unsafe.
    unsafe fn iter_start(&self) -> crate::Result<()>;
    unsafe fn content_unchecked(&self) -> &SmallSet<Value<'v>>;
    unsafe fn iter_stop(&self);
    // fn set_at(&self, index: Hashed<Value<'v>>, value: Value<'v>) -> crate::Result<()>;
}

impl<'v> SetLike<'v> for RefCell<SetData<'v>> {
    type ContentRef<'a>
        = Ref<'a, SmallSet<Value<'v>>>
    where
        Self: 'a,
        'v: 'a;

    fn content<'a>(&'a self) -> Ref<'a, SmallSet<Value<'v>>> {
        Ref::map(self.borrow(), |x| &x.content)
    }

    #[inline]
    unsafe fn iter_start(&self) -> crate::Result<()> {
        // A method may hold this value mutably borrowed while it iterates one of its
        // arguments, e.g. `d.update([d])`; that is an error, not a panic.
        match self.try_borrow() {
            Ok(borrow) => {
                mem::forget(borrow);
                Ok(())
            }
            Err(_) => Err(crate::Error::new_other(ValueError::MutationDuringIteration)),
        }
    }

    #[inline]
    unsafe fn iter_stop(&self) {
        unsafe {
            unleak_borrow(self);
        }
    }

    #[inline]
    unsafe fn content_unchecked(&self) -> &SmallSet<Value<'v>> {
        unsafe { &self.try_borrow_unguarded().ok().unwrap_unchecked().content }
    }
}

/// The frozen half of the pair: `SetGen<SetData<'v>>`. Immutability is structural — there is no
/// `RefCell`, so no caller can obtain the `&mut` the mutators need.
impl<'v> SetLike<'v> for SetData<'v> {
    type ContentRef<'a>
        = &'a SmallSet<Value<'v>>
    where
        Self: 'a,
        'v: 'a;

    fn content(&self) -> &SmallSet<Value<'v>> {
        &self.content
    }

    unsafe fn iter_start(&self) -> crate::Result<()> {
        Ok(())
    }

    unsafe fn iter_stop(&self) {}

    unsafe fn content_unchecked(&self) -> &SmallSet<Value<'v>> {
        &self.content
    }
}

starlark::methods_static!(SET_METHODS = methods::set_methods);

/// A `set` in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub(crate) enum SetView<'v> {
    Frozen(ValueTyped<'v, SetGen<SetData<'v>>>),
    Unfrozen(ValueTyped<'v, SetGen<RefCell<SetData<'v>>>>),
}

/// `$e` with `$x` bound to the representation's `SetLike`.
macro_rules! with_set {
    ($self:expr, |$x:ident| $e:expr) => {
        match $self {
            SetView::Frozen($x) => {
                let $x = &$x.as_ref().0;
                $e
            }
            SetView::Unfrozen($x) => {
                let $x = &$x.as_ref().0;
                $e
            }
        }
    };
}

impl<'v> SetView<'v> {
    #[inline]
    fn content(self) -> Either<Ref<'v, SmallSet<Value<'v>>>, &'v SmallSet<Value<'v>>> {
        match self {
            SetView::Frozen(x) => Either::Right(x.as_ref().0.content()),
            SetView::Unfrozen(x) => Either::Left(x.as_ref().0.content()),
        }
    }
}

#[starlark_value(type = "set")]
impl<'v> StarlarkType<'v> for SetView<'v> {
    /// Returns the length of the value, if this value is a sequence.
    fn length(&self) -> crate::Result<i32> {
        Ok(self.content().len() as i32)
    }

    fn is_in(&self, other: Value<'v>) -> crate::Result<bool> {
        Ok(self.content().contains_hashed(other.get_hashed()?.as_ref()))
    }

    fn equals(&self, other: Value<'v>) -> crate::Result<bool> {
        match SetRef::unpack_value_opt(other) {
            None => Ok(false),
            Some(other) => Ok(equals_small_set(&self.content(), &other.aref.content)),
        }
    }

    fn get_methods() -> Option<&'static Methods> {
        Some(SET_METHODS.methods())
    }

    unsafe fn iterate(&self, me: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        unsafe {
            with_set!(self, |s| s.iter_start())?;
            Ok(me)
        }
    }

    unsafe fn iter_size_hint(&self, index: usize) -> (usize, Option<usize>) {
        let len = self.content().len();
        debug_assert!(index <= len);
        let rem = len - index;
        (rem, Some(rem))
    }

    unsafe fn iter_next(&self, index: usize, _heap: Heap<'v>) -> Option<Value<'v>> {
        unsafe { with_set!(self, |s| s.content_unchecked().iter().nth(index).copied()) }
    }

    unsafe fn iter_stop(&self) {
        unsafe { with_set!(self, |s| s.iter_stop()) }
    }

    fn to_bool(&self) -> bool {
        !self.content().is_empty()
    }

    // Set union
    fn bit_or(&self, rhs: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        // Unlike in `union` it is not possible to `|` `set` and iterable. This is due python semantics.
        let rhs = SetRef::unpack_value_opt(rhs)
            .map_or_else(|| ValueError::unsupported_with(self, "|", rhs), Ok)?;
        let content = self.content();
        if content.is_empty() {
            return Ok(heap.alloc((*rhs.aref).clone()));
        }
        let mut items = content.clone();
        for h in rhs.aref.iter_hashed() {
            items.insert_hashed(h);
        }
        Ok(heap.alloc(SetData { content: items }))
    }

    // Set intersection
    fn bit_and(&self, rhs: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        let rhs = SetRef::unpack_value_opt(rhs)
            .map_or_else(|| ValueError::unsupported_with(self, "&", rhs), Ok)?;

        let mut items = SmallSet::new();
        let content = self.content();
        if content.is_empty() {
            return Ok(heap.alloc(SetData { content: items }));
        }

        for h in rhs.aref.iter_hashed() {
            if content.contains_hashed(h.as_ref()) {
                items.insert_hashed_unique_unchecked(h);
            }
        }

        Ok(heap.alloc(SetData { content: items }))
    }

    // Set symmetric difference
    fn bit_xor(&self, rhs: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        let rhs = SetRef::unpack_value_opt(rhs)
            .map_or_else(|| ValueError::unsupported_with(self, "^", rhs), Ok)?;

        let items = self.content().symmetric_difference(&rhs.aref.content);
        Ok(heap.alloc(SetData { content: items }))
    }

    // Set difference
    //TODO(romanp) implement difference on small_set level and reuse it here and in difference function
    fn sub(&self, rhs: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        let rhs = SetRef::unpack_value_opt(rhs)
            .map_or_else(|| ValueError::unsupported_with(self, "-", rhs), Ok)?;

        let content = self.content();
        if content.is_empty() {
            return Ok(heap.alloc(SetData {
                content: SmallSet::new(),
            }));
        }

        if rhs.aref.content.is_empty() {
            return Ok(heap.alloc(SetData {
                content: content.clone(),
            }));
        }

        let mut data = SetData::default();

        for elem in content.iter_hashed() {
            if !rhs.aref.contains_hashed(elem.copied()) {
                data.add_hashed(elem.copied());
            }
        }
        Ok(heap.alloc(data))
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(Ty::any_set())
    }

    fn get_type_starlark_repr() -> Ty {
        Ty::any_set()
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for SetGen<RefCell<SetData<'v>>> {
    type Canonical = SetGen<SetData<'v>>;
    type Type = SetView<'v>;
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for SetGen<SetData<'v>> {
    type Type = SetView<'v>;
}

impl<'v, T: SetLike<'v>> Serialize for SetGen<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.0.content().iter())
    }
}

impl<'v, T: SetLike<'v>> Display for SetGen<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_container(f, "set([", "])", self.0.content().iter())
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;

    #[test]
    fn test_bit_or() {
        assert::eq("set([1, 2, 3]) | set([3, 4])", "set([1, 2, 3, 4])")
    }

    #[test]
    fn test_bit_or_lhs_empty() {
        assert::eq("set() | set([3, 4])", "set([3, 4])")
    }

    #[test]
    fn test_bit_or_rhs_empty() {
        assert::eq("set([1, 2, 3]) | set()", "set([1, 2, 3])")
    }

    #[test]
    fn test_bit_or_fail_iter() {
        assert::fail(
            "set([1, 2, 3]) | []",
            "Operation `|` not supported for types `set` and `list`",
        );
    }

    #[test]
    fn test_bit_or_ord() {
        assert::eq("list(set([5, 1, 3]) | set([4, 5, 2]))", "[5, 1, 3, 4, 2]")
    }

    #[test]
    fn test_bit_and() {
        assert::eq("set([1, 2, 3]) & set([3, 4])", "set([3])")
    }

    #[test]
    fn test_bit_and_lhs_empty() {
        assert::eq("set() & set([3, 4])", "set([])")
    }

    #[test]
    fn test_bit_and_rhs_empty() {
        assert::eq("set([1, 2, 3]) & set()", "set([])")
    }

    #[test]
    fn test_bit_and_ord() {
        assert::eq("list(set([1, 2, 3]) & set([4, 3, 1]))", "[3, 1]")
    }

    #[test]
    fn test_bit_and_fail_iter() {
        assert::fail(
            "set([1, 2, 3]) & []",
            "Operation `&` not supported for types `set` and `list`",
        );
    }

    #[test]
    fn test_bit_xor() {
        assert::eq("set([1, 2, 3]) ^ set([3, 4])", "set([4, 2, 1])")
    }

    #[test]
    fn test_bit_xor_ord() {
        assert::eq("list(set([1, 2, 3, 7]) ^ set([4, 3, 1]))", "[2, 7, 4]")
    }

    #[test]
    fn test_bit_xor_lhs_empty() {
        assert::eq("set() ^ set([3, 4])", "set([3, 4])")
    }

    #[test]
    fn test_bit_xor_rhs_empty() {
        assert::eq("set([1, 2, 3]) ^ set()", "set([3, 2, 1])")
    }

    #[test]
    fn test_bit_xor_fail_iter() {
        assert::fail(
            "set([1, 2, 3]) ^ []",
            "Operation `^` not supported for types `set` and `list`",
        );
    }

    #[test]
    fn test_sub() {
        assert::eq("set([1, 2, 3]) - set([2])", "set([1, 3])")
    }

    #[test]
    fn test_sub_empty_lhs() {
        assert::eq("set([]) - set([2])", "set([])")
    }

    #[test]
    fn test_sub_empty_rhs() {
        assert::eq("set([1, 2]) - set([])", "set([2, 1])")
    }

    #[test]
    fn test_sub_fail_iter() {
        assert::fail(
            "set([1, 2, 3]) - []",
            "Operation `-` not supported for types `set` and `list`",
        );
    }
}
