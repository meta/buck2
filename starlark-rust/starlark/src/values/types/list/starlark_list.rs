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

use std::convert::Infallible;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::marker::PhantomData;
use std::slice;

use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe_;

use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::array::Array;
use crate::values::error::ValueError;
use crate::values::list::value::FrozenListData;
use crate::values::list::value::ListData;
use crate::values::list::value::ListGen;
use crate::values::list::value::ListRepr;
use crate::values::list::value::VALUE_EMPTY_FROZEN_LIST;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::UnionPayload;

/// A Starlark list. `StarlarkList<'v>` is the type `list`, and `StarlarkList<'v, T>` is
/// `list[T]`, a list whose items are of the type `T`.
///
/// # What to write where
///
/// - A parameter of a native function, the operand of a typed operator, a variant of a
///   `#[derive(UnpackValue)]` union: `StarlarkList<'v, T>`. It is the value read as a list,
///   with its items read as `T`s.
/// - A field that keeps a list: `ValueOf<'v, StarlarkList<'v, T>>`, the checked handle, which
///   is traced and frozen. It has the reading methods of this type, and
///   [`list`](ValueOf::list) gives the `StarlarkList`.
/// - A value that may or may not be a list: [`StarlarkList::from_value`].
/// - The items: [`values`](Self::values) as plain values, [`iter`](Self::iter) as `T`s,
///   [`try_items`](Self::try_items) converted. On a hot path, [`content`](Self::content) is
///   the items as a slice.
///
/// # A list changes
///
/// A `StarlarkList` is cheap to copy and holds no reference to the items: each method looks at
/// the list as it is when the method is called, so a list that is changed while a
/// `StarlarkList` is held is read as changed.
///
/// An iterator over the items keeps the list from being changed while it is alive, as a
/// Starlark `for` loop does: a change is then the error Starlark code gets for changing a
/// list it is iterating. So code that iterates a list and runs Starlark code for each item
/// refuses that code a change to the list, and code that wants to allow it collects the items
/// first.
///
/// The slice that [`content`](Self::content) gives is the one thing that does not follow the
/// list and does not keep it from changing. It borrows the `StarlarkList`, so it cannot be
/// stored or returned; do not run Starlark code while holding it.
///
/// The items were `T`s when the value was read. A list that is not frozen may be given other
/// items afterwards by Starlark code, so what reads an item as a `T` checks it, except in a
/// frozen list.
#[derive(Clone_, Copy_, Dupe_)]
pub struct StarlarkList<'v, T = Value<'v>> {
    repr: ListRepr<'v>,
    _item: PhantomData<fn() -> T>,
}

// SAFETY: the static type is the same type with the heap lifetime erased.
unsafe impl<'v, T: ProvidesStaticType<'v>> ProvidesStaticType<'v> for StarlarkList<'v, T>
where
    T::StaticType: Sized,
{
    type StaticType = StarlarkList<'static, T::StaticType>;
}

impl<T: IsStaticType> IsStaticType for StarlarkList<'static, T>
where
    for<'lt> T::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = StarlarkList<'lt, T::Reinfect<'lt>>;
}

/// `list[T]` for every `T`. At `T = Value` this is the type expression of the type `list`,
/// which is why the list's `StarlarkType` block says `type_expr = by_hand`.
impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for StarlarkList<'v, T> {
    type Get = Self;

    fn starlark_type_repr() -> Ty {
        Ty::list(T::starlark_type_repr())
    }

    #[inline(always)] // A step of a conversion: see `unpack_then`.
    fn get(value: Value<'v>) -> Option<Self> {
        let repr = ListRepr::of_value(value)?;
        Self::items_are_of_the_type(repr).then_some(StarlarkList::of_repr(repr))
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> UnpackValue<'v> for StarlarkList<'v, T> {
    type Type = Self;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, Self>, get: Self) -> Result<Self, Infallible> {
        Ok(get)
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> UnionPayload<'v> for StarlarkList<'v, T> {
    type Expr = Self;

    #[inline]
    fn from_value(value: Value<'v>) -> Option<Self> {
        Self::get(value)
    }
}

impl<'v, T> From<StarlarkList<'v, T>> for Value<'v> {
    #[inline]
    fn from(list: StarlarkList<'v, T>) -> Value<'v> {
        list.repr.to_value()
    }
}

impl<'v> From<ValueTyped<'v, ListGen<FrozenListData<'v>>>> for StarlarkList<'v> {
    #[inline]
    fn from(list: ValueTyped<'v, ListGen<FrozenListData<'v>>>) -> Self {
        StarlarkList::of_repr(ListRepr::Frozen(list))
    }
}

impl<'v> From<ValueTyped<'v, ListGen<ListData<'v>>>> for StarlarkList<'v> {
    #[inline]
    fn from(list: ValueTyped<'v, ListGen<ListData<'v>>>) -> Self {
        StarlarkList::of_repr(ListRepr::Unfrozen(list))
    }
}

impl<'v> StarlarkList<'v> {
    /// `value` as a list, if it is one. For a `list[T]`, which looks at the items, it is
    /// [`StarlarkTypeExpr::get`], or [`ValueOf::new`] for the handle.
    #[inline(always)] // A step of a conversion: see `unpack_then`.
    pub fn from_value(value: Value<'v>) -> Option<Self> {
        ListRepr::of_value(value).map(StarlarkList::of_repr)
    }
}

impl<'v, T> StarlarkList<'v, T> {
    /// A list that is one of its representations. Its items are not looked at: the caller
    /// read them as `T`s, or `T` is `Value`.
    #[inline]
    pub(crate) fn of_repr(repr: ListRepr<'v>) -> Self {
        StarlarkList {
            repr,
            _item: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn repr(self) -> ListRepr<'v> {
        self.repr
    }

    /// The value.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.repr.to_value()
    }

    /// This list as a list of anything, for what takes any list.
    #[inline]
    pub fn untyped(self) -> StarlarkList<'v> {
        StarlarkList::of_repr(self.repr)
    }

    /// The number of items.
    #[inline]
    pub fn len(self) -> usize {
        self.repr.content().len()
    }

    /// Whether the list has no items.
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The items as they are now, as plain values: for a hot path, which an iterator's
    /// bookkeeping would show on.
    ///
    /// The slice does not follow the list and does not keep it from changing: if the list is
    /// changed while the slice is held, the slice may show neither the old items nor the new.
    /// So it borrows the `StarlarkList`, which keeps it from being stored or returned, and
    /// code that holds it runs no Starlark code.
    #[inline]
    pub fn content(&self) -> &[Value<'v>] {
        self.repr.content()
    }

    /// The items, as plain values. The list cannot be changed while the iterator is alive.
    #[inline]
    pub fn values(self) -> ListValues<'v> {
        match self.repr {
            ListRepr::Frozen(list) => ListValues {
                items: list.as_ref().0.content().iter(),
                iterated: None,
            },
            ListRepr::Unfrozen(list) => {
                let array = list.as_ref().0.content.get();
                array.inc_iter_count();
                ListValues {
                    items: array.as_ref().content().iter(),
                    iterated: Some(array),
                }
            }
        }
    }

    /// The list's storage, to change it: an error if the list is frozen or being iterated.
    #[inline]
    pub(crate) fn mutable(self) -> crate::Result<&'v ListData<'v>> {
        match self.repr {
            ListRepr::Unfrozen(list) => {
                let list = &list.as_ref().0;
                list.check_can_mutate()?;
                Ok(list)
            }
            ListRepr::Frozen(_) => Err(cannot_mutate_frozen()),
        }
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> StarlarkList<'v, T> {
    /// Whether every item is a `T` now. Not `#[inline(always)]` as the test of the list in
    /// `get` is: this is a loop, and where it goes is the compiler's to decide.
    #[inline]
    fn items_are_of_the_type(repr: ListRepr<'v>) -> bool {
        repr.content().iter().all(|x| T::matches(*x))
    }

    /// The immutable empty list, which needs no heap.
    #[inline]
    pub fn empty() -> Self {
        Self::of_repr(ListRepr::Frozen(VALUE_EMPTY_FROZEN_LIST.at()))
    }

    /// The handle to this list, for a field or a return.
    #[inline]
    pub fn handle(self) -> ValueOf<'v, Self> {
        // The items were `T`s when this was read, which is all a handle says.
        ValueOf::new_unchecked(self.to_value())
    }

    /// The items. Each is checked to be a `T` as it is read, since the list may have been
    /// changed since it was read as a `list[T]`; in a frozen list nothing is checked and
    /// every item is `Ok`. The list cannot be changed while the iterator is alive.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        let frozen = matches!(self.repr, ListRepr::Frozen(_));
        self.values().map(move |x| {
            if frozen {
                Ok(ValueOf::new_unchecked(x))
            } else {
                ValueOf::new_err(x)
            }
        })
    }

    /// The items, each converted by `U`, a conversion from `T`. Each is checked, as for
    /// [`iter`](Self::iter). The list cannot be changed while the iterator is alive.
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.values().map(U::unpack_value_err)
    }

    /// The items, converted into a `Vec`, as [`try_items`](Self::try_items) gives them.
    pub fn try_to_vec<U>(self) -> crate::Result<Vec<U>>
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.try_items().collect()
    }
}

#[cold]
#[inline(never)]
fn cannot_mutate_frozen() -> crate::Error {
    crate::Error::new_other(ValueError::CannotMutateImmutableValue)
}

impl<'v, T> Debug for StarlarkList<'v, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StarlarkList").field(&self.repr).finish()
    }
}

impl<'v, T> Display for StarlarkList<'v, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.repr.to_value(), f)
    }
}

/// The items of a list as plain values: [`StarlarkList::values`].
///
/// While it is alive the list counts as being iterated, so a change to the list is the error
/// it is in a Starlark `for` loop, and the items the iterator holds stay the list's.
pub struct ListValues<'v> {
    items: slice::Iter<'v, Value<'v>>,
    /// The array of a mutable list, whose count of iterators this holds one of.
    iterated: Option<ValueTyped<'v, Array<'v>>>,
}

impl<'v> Drop for ListValues<'v> {
    #[inline]
    fn drop(&mut self) {
        if let Some(array) = self.iterated {
            array.dec_iter_count();
        }
    }
}

impl<'v> Iterator for ListValues<'v> {
    type Item = Value<'v>;

    #[inline]
    fn next(&mut self) -> Option<Value<'v>> {
        self.items.next().copied()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.items.size_hint()
    }
}

impl<'v> ExactSizeIterator for ListValues<'v> {}

/// The handle to a list reads it as the list does: these are [`StarlarkList`]'s methods of
/// the same names, so that a field is read without a step in between.
impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, StarlarkList<'v, T>> {
    /// The list. Its items are not checked here: what reads them as `T`s checks each, as
    /// [`StarlarkList::iter`] says. ([`try_get`](ValueOf::try_get) is this with every item
    /// checked first.)
    #[inline]
    pub fn list(self) -> StarlarkList<'v, T> {
        StarlarkList::of_repr(
            ListRepr::of_value(self.to_value()).expect("a value's type does not change"),
        )
    }

    /// [`StarlarkList::len`].
    #[inline]
    pub fn len(self) -> usize {
        self.list().len()
    }

    /// [`StarlarkList::is_empty`].
    #[inline]
    pub fn is_empty(self) -> bool {
        self.list().is_empty()
    }

    /// [`StarlarkList::content`].
    #[inline]
    pub fn content(&self) -> &[Value<'v>] {
        self.list().repr.content()
    }

    /// [`StarlarkList::values`].
    #[inline]
    pub fn values(self) -> ListValues<'v> {
        self.list().values()
    }

    /// [`StarlarkList::iter`].
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        self.list().iter()
    }

    /// [`StarlarkList::try_items`].
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.list().try_items()
    }

    /// [`StarlarkList::try_to_vec`].
    pub fn try_to_vec<U>(self) -> crate::Result<Vec<U>>
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.list().try_to_vec()
    }
}

#[cfg(test)]
mod tests {
    use crate::values::Heap;
    use crate::values::StarlarkTypeExpr;
    use crate::values::UnpackValue;
    use crate::values::Value;
    use crate::values::ValueOf;
    use crate::values::int::IntView;
    use crate::values::list::AllocList;
    use crate::values::list::StarlarkList;
    use crate::values::string::StarlarkStr;

    #[test]
    fn test_reading() {
        Heap::temp(|heap| {
            let list = heap.alloc(AllocList([1, 2, 3])).to_value();
            let (handle, reading) = ValueOf::<StarlarkList<IntView>>::new_with_get(list).unwrap();
            assert!(reading.to_value().ptr_eq(list));
            assert!(reading.handle().to_value().ptr_eq(list));
            assert_eq!(3, reading.len());
            assert_eq!(3, reading.iter().count());
            assert_eq!(vec![1, 2, 3], reading.try_to_vec::<i32>().unwrap());
            // The handle reads the list as the list does.
            assert_eq!(3, handle.len());
            assert_eq!(vec![1, 2, 3], handle.try_to_vec::<i32>().unwrap());
            assert!(std::ptr::eq(handle.content(), handle.list().content()));
            // A parameter spelled as the type is the reading.
            let unpacked = StarlarkList::from_value(list).unwrap();
            assert!(std::ptr::eq(reading.content(), unpacked.content()));
            assert!(<StarlarkList<StarlarkStr>>::unpack_value_opt(list).is_none());
        });
    }

    /// `StarlarkList<'v>` is the type `list` and `list[typing.Any]`; they are one type.
    #[test]
    fn test_the_list_type_and_a_list_of_anything_are_one() {
        assert_eq!(
            <StarlarkList as StarlarkTypeExpr>::starlark_type_repr(),
            <StarlarkList as crate::values::StarlarkType>::get_type_starlark_repr(),
        );
        Heap::temp(|heap| {
            let list = heap.alloc(AllocList(["a"])).to_value();
            let typed = <StarlarkList<StarlarkStr>>::unpack_value_opt(list).unwrap();
            assert!(typed.untyped().to_value().ptr_eq(list));
            assert!(ValueOf::<StarlarkList>::new(list).is_some());
        });
    }

    /// A reading that is held does not keep the list from changing and reads it as changed:
    /// a list that grows moves its items to a new array, which a reference to the items would
    /// not see. An iterator does keep the list from changing, and the change is an error.
    #[test]
    fn test_reading_and_changing() {
        Heap::temp(|heap| {
            let list = heap.alloc(AllocList([1])).to_value();
            let reading = <StarlarkList<IntView>>::unpack_value_opt(list).unwrap();
            let storage = reading.mutable().unwrap();
            for i in 0..100i32 {
                storage.push(heap.alloc(i).to_value(), heap);
            }
            assert_eq!(101, reading.len());
            assert_eq!(
                Some(99),
                reading.content().last().and_then(|x| x.unpack_i32())
            );
            {
                let mut values = reading.values();
                assert_eq!(Some(1), values.next().and_then(Value::unpack_i32));
                let e = reading.mutable().err().unwrap();
                assert!(e.to_string().contains("mutates an iterable"), "{e}");
                assert_eq!(100, values.len());
            }
            // An item that is no longer a `T` is an error where it is read, not a wrong `T`.
            reading
                .mutable()
                .unwrap()
                .push(heap.alloc("s").to_value(), heap);
            assert!(reading.iter().last().unwrap().is_err());
            assert!(reading.try_to_vec::<i32>().is_err());
        });
    }
}
