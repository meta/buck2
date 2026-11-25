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
use dupe::Dupe;
use either::Either;
use serde::Serialize;
use starlark_map::Hashed;
use starlark_map::small_set::SmallSet;

use super::refs::SetMut;
use super::refs::SetRef;
use crate as starlark;
use crate::typing::Ty;
use crate::util::refcell::unleak_borrow;
use crate::values::AllocValue;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::Heap;
use crate::values::ProvidesStaticType;
use crate::values::StarlarkPagable;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::Trace;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::comparison::equals_small_set;
use crate::values::function::SpecialBuiltinFunction;
use crate::values::set::StarlarkSet;
use crate::values::set::methods;
use crate::values::starlark_value;
use crate::values::typing::StarlarkIter;

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
    type Type = StarlarkSet<'v>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::alloc(heap, SetGen(RefCell::new(self)))
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

/// Which of its representations a `set` is.
#[derive(Debug, Clone, Copy, Dupe)]
pub(crate) enum SetRepr<'v> {
    Frozen(ValueTyped<'v, SetGen<SetData<'v>>>),
    Unfrozen(ValueTyped<'v, SetGen<RefCell<SetData<'v>>>>),
}

/// `$e` with `$x` bound to the representation's `SetLike`.
macro_rules! with_set {
    ($self:expr, |$x:ident| $e:expr) => {
        match $self {
            SetRepr::Frozen($x) => {
                let $x = &$x.as_ref().0;
                $e
            }
            SetRepr::Unfrozen($x) => {
                let $x = &$x.as_ref().0;
                $e
            }
        }
    };
}

impl<'v> SetRepr<'v> {
    /// `value` as a set, if it is one. The representation is told by whether the value is
    /// frozen, so one representation is tested, not both in turn.
    #[inline(always)] // A step of a conversion: see `unpack_then`.
    pub(crate) fn of_value(value: Value<'v>) -> Option<Self> {
        if value.is_frozen() {
            ValueTyped::new(value).map(SetRepr::Frozen)
        } else {
            ValueTyped::new(value).map(SetRepr::Unfrozen)
        }
    }

    #[inline]
    pub(crate) fn to_value(self) -> Value<'v> {
        match self {
            SetRepr::Frozen(x) => x.to_value(),
            SetRepr::Unfrozen(x) => x.to_value(),
        }
    }

    #[inline]
    pub(crate) fn is_frozen(self) -> bool {
        matches!(self, SetRepr::Frozen(_))
    }

    /// The set, borrowed for as long as the reference lives if it is mutable.
    #[inline]
    pub(crate) fn to_ref(self) -> SetRef<'v> {
        SetRef {
            aref: match self {
                SetRepr::Frozen(x) => Either::Right(&x.as_ref().0),
                SetRepr::Unfrozen(x) => Either::Left(x.as_ref().0.borrow()),
            },
        }
    }

    /// The set, to change it: an error if it is frozen or being iterated.
    #[inline]
    pub(crate) fn to_mut(self) -> crate::Result<SetMut<'v>> {
        match self {
            SetRepr::Unfrozen(x) => match x.as_ref().0.try_borrow_mut() {
                Ok(aref) => Ok(SetMut { aref }),
                Err(_) => Err(cannot_mutate(ValueError::MutationDuringIteration)),
            },
            SetRepr::Frozen(_) => Err(cannot_mutate(ValueError::CannotMutateImmutableValue)),
        }
    }

    #[inline]
    fn content(self) -> Either<Ref<'v, SmallSet<Value<'v>>>, &'v SmallSet<Value<'v>>> {
        match self {
            SetRepr::Frozen(x) => Either::Right(x.as_ref().0.content()),
            SetRepr::Unfrozen(x) => Either::Left(x.as_ref().0.content()),
        }
    }
}

#[cold]
#[inline(never)]
fn cannot_mutate(error: ValueError) -> crate::Error {
    crate::Error::new_other(error)
}

#[starlark_value(type = "set", type_expr = by_hand, methods = methods::set_methods)]
impl<'v> StarlarkType<'v> for StarlarkSet<'v> {
    #[starlark(
        constructor,
        speculative_exec_safe,
        special_builtin_function = SpecialBuiltinFunction::Set,
    )]
    fn set(
        #[starlark(require = pos)] arg: Option<ValueOf<'v, StarlarkIter<Value<'v>>>>,
        heap: Heap<'v>,
    ) -> starlark::Result<SetData<'v>> {
        let set = match arg {
            Some(pos) => match StarlarkSet::from_value(pos.to_value()) {
                Some(set) => (set.borrowed().aref).clone(),
                None => {
                    let it = pos.to_value().iterate(heap)?;
                    let mut data = SetData::default();
                    for el in it {
                        let el = el.get_hashed()?;
                        data.content.insert_hashed(el);
                    }
                    data
                }
            },
            None => SetData::default(),
        };
        Ok(set)
    }

    /// Returns the length of the value, if this value is a sequence.
    fn length(this: Self) -> i32 {
        this.repr().content().len() as i32
    }

    fn is_in(this: Self, other: Value<'v>) -> crate::Result<bool> {
        Ok(this
            .repr()
            .content()
            .contains_hashed(other.get_hashed()?.as_ref()))
    }

    // The operand is a plain value read with `from_value`, not a typed `Self`: read as a
    // `set[typing.Any]` it is borrowed once more, to look at items that there is nothing to
    // check about. Inlined into each representation's entry, as the macro does by itself for
    // a typed method.
    #[inline(always)]
    fn equals(this: Self, other: Value<'v>) -> crate::Result<bool> {
        match StarlarkSet::from_value(other) {
            Some(other) => Ok(equals_small_set(
                &this.repr().content(),
                &other.repr().content(),
            )),
            None => Ok(false),
        }
    }

    unsafe fn iterate(this: Self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        unsafe {
            with_set!(this.repr(), |s| s.iter_start())?;
            Ok(this.to_value())
        }
    }

    unsafe fn iter_size_hint(this: Self, index: usize) -> (usize, Option<usize>) {
        let len = this.repr().content().len();
        debug_assert!(index <= len);
        let rem = len - index;
        (rem, Some(rem))
    }

    unsafe fn iter_next(this: Self, index: usize, _heap: Heap<'v>) -> Option<Value<'v>> {
        unsafe {
            with_set!(this.repr(), |s| s
                .content_unchecked()
                .iter()
                .nth(index)
                .copied())
        }
    }

    unsafe fn iter_stop(this: Self) {
        unsafe { with_set!(this.repr(), |s| s.iter_stop()) }
    }

    fn to_bool(this: Self) -> bool {
        !this.repr().content().is_empty()
    }

    // Set union
    // Unlike in `union` it is not possible to `|` `set` and iterable. This is due python semantics.
    fn bit_or(this: Self, rhs: Self) -> SetData<'v> {
        let rhs = rhs.borrowed();
        let content = this.repr().content();
        if content.is_empty() {
            return (*rhs.aref).clone();
        }
        let mut items = content.clone();
        for h in rhs.aref.iter_hashed() {
            items.insert_hashed(h);
        }
        SetData { content: items }
    }

    // Set intersection
    fn bit_and(this: Self, rhs: Self) -> SetData<'v> {
        let rhs = rhs.borrowed();
        let mut items = SmallSet::new();
        let content = this.repr().content();
        if content.is_empty() {
            return SetData { content: items };
        }

        for h in rhs.aref.iter_hashed() {
            if content.contains_hashed(h.as_ref()) {
                items.insert_hashed_unique_unchecked(h);
            }
        }

        SetData { content: items }
    }

    // Set symmetric difference
    fn bit_xor(this: Self, rhs: Self) -> SetData<'v> {
        let rhs = rhs.borrowed();
        let items = this
            .repr()
            .content()
            .symmetric_difference(&rhs.aref.content);
        SetData { content: items }
    }

    // Set difference
    fn sub(this: Self, rhs: Self) -> SetData<'v> {
        let rhs = rhs.borrowed();
        let content = this.repr().content();

        let content = content.difference_with(&rhs.aref.content);
        SetData { content }
    }

    fn get_type_starlark_repr() -> Ty {
        Ty::any_set()
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for SetGen<RefCell<SetData<'v>>> {
    type Type = StarlarkSet<'v>;
}

#[starlark_value(alloc = by_hand)]
impl<'v> StarlarkValue<'v> for SetGen<SetData<'v>> {
    type Type = StarlarkSet<'v>;
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
