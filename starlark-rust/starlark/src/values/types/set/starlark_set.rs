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

use std::cell::RefCell;
use std::convert::Infallible;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::iter;
use std::marker::PhantomData;

use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe_;
use starlark_map::small_set;
use starlark_map::small_set::SmallSet;

use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::set::SetMut;
use crate::values::set::SetRef;
use crate::values::set::value::SetData;
use crate::values::set::value::SetGen;
use crate::values::set::value::SetRepr;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;
use crate::values::type_expr::UnionPayload;

/// A Starlark set. `StarlarkSet<'v>` is the type `set`, and `StarlarkSet<'v, T>` is `set[T]`,
/// a set whose items are of the type `T`.
///
/// # What to write where
///
/// - A parameter of a native function, the operand of a typed operator, a variant of a
///   `#[derive(UnpackValue)]` union: `StarlarkSet<'v, T>`. It is the value read as a set, with
///   its items read as `T`s.
/// - A field that keeps a set: `ValueOf<'v, StarlarkSet<'v, T>>`, the checked handle, which
///   is traced and frozen. It has the reading methods of this type, and
///   [`set`](ValueOf::set) gives the `StarlarkSet`.
/// - A value that may or may not be a set: [`StarlarkSet::from_value`].
/// - The items: [`values`](Self::values) as plain values, [`iter`](Self::iter) as `T`s,
///   [`try_items`](Self::try_items) converted; [`contains`](Self::contains) for one.
///
/// # A set changes
///
/// A `StarlarkSet` is cheap to copy and holds no borrow of the set. Each method reads or
/// changes the set as it is when the method is called and is done with it when it returns,
/// so a `StarlarkSet` can be held across anything, Starlark code included, and a set that is
/// changed while one is held is read as changed.
///
/// An iterator over the items keeps the set from being changed while it is alive, as a
/// Starlark `for` loop does: [`insert`](Self::insert), [`remove`](Self::remove) and
/// [`clear`](Self::clear) are then the error Starlark code gets for changing a set it is
/// iterating.
///
/// The items were `T`s when the value was read. A set that is not frozen may be given other
/// items afterwards by Starlark code, so what reads an item as a `T` checks it, except in a
/// frozen set.
#[derive(Clone_, Copy_, Dupe_)]
pub struct StarlarkSet<'v, T = Value<'v>> {
    repr: SetRepr<'v>,
    _item: PhantomData<fn() -> T>,
}

// SAFETY: the static type is the same type with the heap lifetime erased.
unsafe impl<'v, T: ProvidesStaticType<'v>> ProvidesStaticType<'v> for StarlarkSet<'v, T>
where
    T::StaticType: Sized,
{
    type StaticType = StarlarkSet<'static, T::StaticType>;
}

impl<T: IsStaticType> IsStaticType for StarlarkSet<'static, T>
where
    for<'lt> T::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = StarlarkSet<'lt, T::Reinfect<'lt>>;
}

/// `set[T]` for every `T`. At `T = Value` this is the type expression of the type `set`,
/// which is why the set's `StarlarkType` block says `type_expr = by_hand`.
impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for StarlarkSet<'v, T> {
    type Get = Self;

    fn starlark_type_repr() -> Ty {
        Ty::set(T::starlark_type_repr())
    }

    #[inline]
    fn get(value: Value<'v>) -> Option<Self> {
        let repr = SetRepr::of_value(value)?;
        repr.to_ref()
            .iter()
            .all(|x| T::matches(x))
            .then_some(StarlarkSet::of_repr(repr))
    }
}

/// Every value is a `typing.Any`, so no change to the set's items can invalidate the check.
impl<'v> StarlarkTypeExprStable<'v> for StarlarkSet<'v> {}

impl<'v, T: StarlarkTypeExpr<'v>> UnpackValue<'v> for StarlarkSet<'v, T> {
    type Type = Self;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, Self>, get: Self) -> Result<Self, Infallible> {
        Ok(get)
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> UnionPayload<'v> for StarlarkSet<'v, T> {
    type Expr = Self;

    #[inline]
    fn from_value(value: Value<'v>) -> Option<Self> {
        Self::get(value)
    }
}

impl<'v, T> From<StarlarkSet<'v, T>> for Value<'v> {
    #[inline]
    fn from(set: StarlarkSet<'v, T>) -> Value<'v> {
        set.repr.to_value()
    }
}

impl<'v> From<ValueTyped<'v, SetGen<SetData<'v>>>> for StarlarkSet<'v> {
    #[inline]
    fn from(set: ValueTyped<'v, SetGen<SetData<'v>>>) -> Self {
        StarlarkSet::of_repr(SetRepr::Frozen(set))
    }
}

impl<'v> From<ValueTyped<'v, SetGen<RefCell<SetData<'v>>>>> for StarlarkSet<'v> {
    #[inline]
    fn from(set: ValueTyped<'v, SetGen<RefCell<SetData<'v>>>>) -> Self {
        StarlarkSet::of_repr(SetRepr::Unfrozen(set))
    }
}

impl<'v> StarlarkSet<'v> {
    /// `value` as a set, if it is one. For a `set[T]`, which looks at the items, it is
    /// [`StarlarkTypeExpr::get`], or [`ValueOf::new`] for the handle.
    #[inline(always)] // A step of a conversion: see `unpack_then`.
    pub fn from_value(value: Value<'v>) -> Option<Self> {
        SetRepr::of_value(value).map(StarlarkSet::of_repr)
    }
}

impl<'v, T> StarlarkSet<'v, T> {
    /// A set that is one of its representations. Its items are not looked at: the caller
    /// read them as `T`s, or `T` is `Value`.
    #[inline]
    pub(crate) fn of_repr(repr: SetRepr<'v>) -> Self {
        StarlarkSet {
            repr,
            _item: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn repr(self) -> SetRepr<'v> {
        self.repr
    }

    /// The set's items, borrowed until the reference is dropped: the set cannot be changed
    /// meanwhile. Not public: what is public borrows inside one call, or inside an iterator.
    #[inline]
    pub(crate) fn borrowed(self) -> SetRef<'v> {
        self.repr.to_ref()
    }

    /// The set's items, to change them, until the reference is dropped: the set cannot be
    /// read meanwhile, and reading it panics. An error if the set is frozen or being
    /// iterated. Not public, for that panic: the crate's own code holds it across nothing
    /// that could read the set.
    #[inline]
    pub(crate) fn mutable(self) -> crate::Result<SetMut<'v>> {
        self.repr.to_mut()
    }

    /// The value.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.repr.to_value()
    }

    /// This set as a set of anything, for what takes any set.
    #[inline]
    pub fn untyped(self) -> StarlarkSet<'v> {
        StarlarkSet::of_repr(self.repr)
    }

    /// The number of items.
    #[inline]
    pub fn len(self) -> usize {
        self.borrowed().aref.content.len()
    }

    /// Whether the set has no items.
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Whether `value` is in the set. An error if `value` does not hash.
    #[inline]
    pub fn contains(self, value: Value<'v>) -> crate::Result<bool> {
        let value = value.get_hashed()?;
        Ok(self.borrowed().aref.contains_hashed(value))
    }

    /// The items, as plain values. The set cannot be changed while the iterator is alive.
    #[inline]
    pub fn values(self) -> SetValues<'v> {
        let borrow = self.borrowed();
        let content: *const SmallSet<Value<'v>> = &borrow.aref.content;
        // SAFETY: the set is in the heap, which lives for `'v`, and it is not changed while
        // it is borrowed; the iterator keeps the borrow for as long as it has the reference.
        let content: &'v SmallSet<Value<'v>> = unsafe { &*content };
        SetValues {
            items: content.iter().copied(),
            _borrow: borrow,
        }
    }

    /// Removes every item. An error if the set is frozen or being iterated.
    #[inline]
    pub fn clear(self) -> crate::Result<()> {
        self.mutable()?.aref.clear();
        Ok(())
    }

    /// Removes `value`, and says whether it was in the set. An error if the set is frozen,
    /// if it is being iterated, or if `value` does not hash.
    #[inline]
    pub fn remove(self, value: Value<'v>) -> crate::Result<bool> {
        let value = value.get_hashed()?;
        Ok(self.mutable()?.aref.remove_hashed(value.as_ref()))
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> StarlarkSet<'v, T> {
    /// The handle to this set, for a field or a return.
    #[inline]
    pub fn handle(self) -> ValueOf<'v, Self> {
        // The items were `T`s when this was read, which is all a handle says.
        ValueOf::new_unchecked(self.to_value())
    }

    /// The items. Each is checked to be a `T` as it is read, since the set may have been
    /// changed since it was read as a `set[T]`; in a frozen set nothing is checked and every
    /// item is `Ok`. The set cannot be changed while the iterator is alive.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v
    where
        T: 'v,
    {
        let frozen = self.repr.is_frozen();
        self.values().map(move |x| {
            if frozen {
                Ok(ValueOf::new_unchecked(x))
            } else {
                ValueOf::new_err(x)
            }
        })
    }

    /// The items, each converted by `U`, a conversion from `T`. Each is checked, as for
    /// [`iter`](Self::iter). The set cannot be changed while the iterator is alive.
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.values().map(U::unpack_value_err)
    }

    /// Adds `value`, and says whether it was not in the set already. An error if the set is
    /// frozen, if it is being iterated, or if `value` does not hash.
    #[inline]
    pub fn insert(self, value: ValueOf<'v, T>) -> crate::Result<bool> {
        let value = value.to_value().get_hashed()?;
        Ok(self.mutable()?.aref.add_hashed(value))
    }
}

impl<'v, T> Debug for StarlarkSet<'v, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StarlarkSet").field(&self.repr).finish()
    }
}

impl<'v, T> Display for StarlarkSet<'v, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.repr.to_value(), f)
    }
}

/// The items of a set as plain values: [`StarlarkSet::values`].
///
/// While it is alive the set is borrowed, so a change to the set is the error it is in a
/// Starlark `for` loop.
pub struct SetValues<'v> {
    items: iter::Copied<small_set::Iter<'v, Value<'v>>>,
    /// Keeps the set from being changed under `items`.
    _borrow: SetRef<'v>,
}

impl<'v> Iterator for SetValues<'v> {
    type Item = Value<'v>;

    #[inline]
    fn next(&mut self) -> Option<Value<'v>> {
        self.items.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.items.size_hint()
    }
}

impl<'v> ExactSizeIterator for SetValues<'v> {}

/// The handle to a set reads it as the set does: these are [`StarlarkSet`]'s methods of the
/// same names, so that a field is read without a step in between.
impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, StarlarkSet<'v, T>> {
    /// The set. Its items are not checked here: what reads them as `T`s checks each, as
    /// [`StarlarkSet::iter`] says. ([`try_get`](ValueOf::try_get) is this with every item
    /// checked first.)
    #[inline]
    pub fn set(self) -> StarlarkSet<'v, T> {
        StarlarkSet::of_repr(
            SetRepr::of_value(self.to_value()).expect("a value's type does not change"),
        )
    }

    /// [`StarlarkSet::len`].
    #[inline]
    pub fn len(self) -> usize {
        self.set().len()
    }

    /// [`StarlarkSet::is_empty`].
    #[inline]
    pub fn is_empty(self) -> bool {
        self.set().is_empty()
    }

    /// [`StarlarkSet::values`].
    #[inline]
    pub fn values(self) -> SetValues<'v> {
        self.set().values()
    }

    /// [`StarlarkSet::iter`].
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v
    where
        T: 'v,
    {
        self.set().iter()
    }

    /// [`StarlarkSet::try_items`].
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.set().try_items()
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;
    use crate::values::Heap;
    use crate::values::StarlarkTypeExpr;
    use crate::values::UnpackValue;
    use crate::values::ValueOf;
    use crate::values::int::IntView;
    use crate::values::set::StarlarkSet;
    use crate::values::set::value::SetData;
    use crate::values::string::StarlarkStr;

    #[test]
    fn test_reading() {
        let set = assert::pass("set([1, 2, 3])");
        let set = set.as_ref().value();
        let (handle, reading) = ValueOf::<StarlarkSet<IntView>>::new_with_get(set).unwrap();
        assert!(reading.to_value().ptr_eq(set));
        assert!(reading.handle().to_value().ptr_eq(set));
        assert_eq!(3, reading.len());
        assert_eq!(3, reading.values().count());
        assert_eq!(3, reading.iter().count());
        // The handle reads the set as the set does.
        assert_eq!(3, handle.len());
        assert_eq!(3, handle.iter().count());
        // A parameter spelled as the type is the reading.
        assert_eq!(3, StarlarkSet::from_value(set).unwrap().len());
        assert!(<StarlarkSet<StarlarkStr>>::unpack_value_opt(set).is_none());
    }

    /// `StarlarkSet<'v>` is the type `set` and `set[typing.Any]`; they are one type.
    #[test]
    fn test_the_set_type_and_a_set_of_anything_are_one() {
        assert_eq!(
            <StarlarkSet as StarlarkTypeExpr>::starlark_type_repr(),
            <StarlarkSet as crate::values::StarlarkType>::get_type_starlark_repr(),
        );
    }

    /// A reading that is held does not keep the set from changing and reads it as changed.
    /// An iterator does keep it, and the change is an error, not a panic.
    #[test]
    fn test_reading_and_changing() {
        Heap::temp(|heap| {
            let set = heap.alloc(SetData::default()).to_value();
            let reading = <StarlarkSet<IntView>>::unpack_value_opt(set).unwrap();
            let one = ValueOf::alloc(heap, 1);
            assert!(reading.insert(one).unwrap());
            assert!(!reading.insert(one).unwrap());
            assert!(reading.contains(one.to_value()).unwrap());
            assert_eq!(1, reading.len());
            {
                let mut values = reading.values();
                assert!(values.next().is_some());
                let e = reading.insert(one).err().unwrap();
                assert!(e.to_string().contains("mutates an iterable"), "{e}");
                let e = reading.clear().err().unwrap();
                assert!(e.to_string().contains("mutates an iterable"), "{e}");
            }
            // An item that is no longer a `T` is an error where it is read.
            reading
                .untyped()
                .insert(ValueOf::new_unchecked(heap.alloc("s").to_value()))
                .unwrap();
            assert!(reading.iter().last().unwrap().is_err());
            assert!(reading.remove(one.to_value()).unwrap());
            reading.clear().unwrap();
            assert!(reading.is_empty());
        });
    }
}
