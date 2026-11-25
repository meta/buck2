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

use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe_;
use starlark_map::small_map;

use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::collections::Hashed;
use crate::collections::SmallMap;
use crate::typing::Ty;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::dict::Dict;
use crate::values::dict::DictMut;
use crate::values::dict::DictRef;
use crate::values::dict::value::DictGen;
use crate::values::dict::value::DictRepr;
use crate::values::dict::value::VALUE_EMPTY_FROZEN_DICT;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;
use crate::values::type_expr::UnionPayload;

/// A Starlark dict. `StarlarkDict<'v>` is the type `dict`, and `StarlarkDict<'v, K, V>` is
/// `dict[K, V]`, a dict whose keys are of the type `K` and whose values are of the type `V`.
///
/// # What to write where
///
/// - A parameter of a native function, the operand of a typed operator, a variant of a
///   `#[derive(UnpackValue)]` union: `StarlarkDict<'v, K, V>`. It is the value read as a
///   dict, with its entries read as `K`s and `V`s.
/// - A field that keeps a dict: `ValueOf<'v, StarlarkDict<'v, K, V>>`, the checked handle,
///   which is traced and frozen. It has the reading methods of this type, and
///   [`dict`](ValueOf::dict) gives the `StarlarkDict`.
/// - A value that may or may not be a dict: [`StarlarkDict::from_value`].
/// - One entry: [`get`](Self::get), [`get_str`](Self::get_str) as a plain value,
///   [`lookup`](Self::lookup) as a `V`.
/// - The entries: [`entries`](Self::entries), [`keys`](Self::keys) and
///   [`values`](Self::values) as plain values, [`iter`](Self::iter) as `K`s and `V`s,
///   [`try_items`](Self::try_items) converted.
///
/// # A dict changes
///
/// A `StarlarkDict` is cheap to copy and holds no borrow of the dict. Each method reads or
/// changes the dict as it is when the method is called and is done with it when it returns,
/// so a `StarlarkDict` can be held across anything, Starlark code included, and a dict that
/// is changed while one is held is read as changed.
///
/// An iterator over the entries keeps the dict from being changed while it is alive, as a
/// Starlark `for` loop does: [`insert`](Self::insert), [`remove`](Self::remove) and
/// [`clear`](Self::clear) are then the error Starlark code gets for changing a dict it is
/// iterating. So code that iterates a dict and runs Starlark code for each entry refuses that
/// code a change to the dict, and code that iterates and changes the dict in turns collects
/// the entries first.
///
/// The entries were `K`s and `V`s when the value was read. A dict that is not frozen may be
/// given other entries afterwards by Starlark code, so what reads an entry as a `K` or a `V`
/// checks it, except in a frozen dict.
#[derive(Clone_, Copy_, Dupe_)]
pub struct StarlarkDict<'v, K = Value<'v>, V = Value<'v>> {
    repr: DictRepr<'v>,
    _entry: PhantomData<fn() -> (K, V)>,
}

// SAFETY: the static type is the same type with the heap lifetime erased.
unsafe impl<'v, K: ProvidesStaticType<'v>, V: ProvidesStaticType<'v>> ProvidesStaticType<'v>
    for StarlarkDict<'v, K, V>
where
    K::StaticType: Sized,
    V::StaticType: Sized,
{
    type StaticType = StarlarkDict<'static, K::StaticType, V::StaticType>;
}

impl<K: IsStaticType, V: IsStaticType> IsStaticType for StarlarkDict<'static, K, V>
where
    for<'lt> K::Reinfect<'lt>: Sized,
    for<'lt> V::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = StarlarkDict<'lt, K::Reinfect<'lt>, V::Reinfect<'lt>>;
}

/// `dict[K, V]` for every `K` and `V`. At `Value` for both this is the type expression of
/// the type `dict`, which is why the dict's `StarlarkType` block says `type_expr = by_hand`.
impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v>
    for StarlarkDict<'v, K, V>
{
    type Get = Self;

    fn starlark_type_repr() -> Ty {
        Ty::dict(K::starlark_type_repr(), V::starlark_type_repr())
    }

    #[inline]
    fn get(value: Value<'v>) -> Option<Self> {
        let repr = DictRepr::of_value(value)?;
        repr.to_ref()
            .iter()
            .all(|(k, v)| K::matches(k) && V::matches(v))
            .then_some(StarlarkDict::of_repr(repr))
    }
}

/// Every value is a `typing.Any`, so no change to the dict's entries can invalidate the check.
impl<'v> StarlarkTypeExprStable<'v> for StarlarkDict<'v> {}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> UnpackValue<'v>
    for StarlarkDict<'v, K, V>
{
    type Type = Self;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, Self>, get: Self) -> Result<Self, Infallible> {
        Ok(get)
    }
}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> UnionPayload<'v>
    for StarlarkDict<'v, K, V>
{
    type Expr = Self;

    #[inline]
    fn from_value(value: Value<'v>) -> Option<Self> {
        // Not `Self::get`, which is the dict's lookup of a key.
        <Self as StarlarkTypeExpr<'v>>::get(value)
    }
}

impl<'v, K, V> From<StarlarkDict<'v, K, V>> for Value<'v> {
    #[inline]
    fn from(dict: StarlarkDict<'v, K, V>) -> Value<'v> {
        dict.repr.to_value()
    }
}

impl<'v> From<ValueTyped<'v, DictGen<Dict<'v>>>> for StarlarkDict<'v> {
    #[inline]
    fn from(dict: ValueTyped<'v, DictGen<Dict<'v>>>) -> Self {
        StarlarkDict::of_repr(DictRepr::Frozen(dict))
    }
}

impl<'v> From<ValueTyped<'v, DictGen<std::cell::RefCell<Dict<'v>>>>> for StarlarkDict<'v> {
    #[inline]
    fn from(dict: ValueTyped<'v, DictGen<std::cell::RefCell<Dict<'v>>>>) -> Self {
        StarlarkDict::of_repr(DictRepr::Unfrozen(dict))
    }
}

impl<'v> StarlarkDict<'v> {
    /// `value` as a dict, if it is one. For a `dict[K, V]`, which looks at the entries, it is
    /// [`StarlarkTypeExpr::get`], or [`ValueOf::new`] for the handle.
    #[inline(always)] // A step of a conversion: see `unpack_then`.
    pub fn from_value(value: Value<'v>) -> Option<Self> {
        DictRepr::of_value(value).map(StarlarkDict::of_repr)
    }
}

impl<'v, K, V> StarlarkDict<'v, K, V> {
    /// A dict that is one of its representations. Its entries are not looked at: the caller
    /// read them as `K`s and `V`s, or both are `Value`.
    #[inline]
    pub(crate) fn of_repr(repr: DictRepr<'v>) -> Self {
        StarlarkDict {
            repr,
            _entry: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn repr(self) -> DictRepr<'v> {
        self.repr
    }

    /// The dict's entries, borrowed until the reference is dropped: the dict cannot be
    /// changed meanwhile. Not public: what is public borrows inside one call, or inside an
    /// iterator.
    #[inline]
    pub(crate) fn borrowed(self) -> DictRef<'v> {
        self.repr.to_ref()
    }

    /// The dict's entries, to change them, until the reference is dropped: the dict cannot
    /// be read meanwhile, and reading it panics. An error if the dict is frozen or being
    /// iterated. Not public, for that panic: the crate's own code holds it across nothing
    /// that could read the dict.
    #[inline]
    pub(crate) fn mutable(self) -> crate::Result<DictMut<'v>> {
        self.repr.to_mut()
    }

    /// The value.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.repr.to_value()
    }

    /// This dict as a dict of anything, for what takes any dict.
    #[inline]
    pub fn untyped(self) -> StarlarkDict<'v> {
        StarlarkDict::of_repr(self.repr)
    }

    /// The number of entries.
    #[inline]
    pub fn len(self) -> usize {
        self.borrowed().len()
    }

    /// Whether the dict has no entries.
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The value at `key`, as a plain value. An error if `key` does not hash.
    #[inline]
    pub fn get(self, key: Value<'v>) -> crate::Result<Option<Value<'v>>> {
        Ok(self.get_hashed(key.get_hashed()?))
    }

    /// The value at a key whose hash is known, as a plain value.
    #[inline]
    pub fn get_hashed(self, key: Hashed<Value<'v>>) -> Option<Value<'v>> {
        self.borrowed().get_hashed(key)
    }

    /// The value at a string key, as a plain value.
    #[inline]
    pub fn get_str(self, key: &str) -> Option<Value<'v>> {
        self.borrowed().get_str(key)
    }

    /// The entries, as plain values. The dict cannot be changed while the iterator is alive.
    #[inline]
    pub fn entries(self) -> DictValues<'v> {
        DictValues::of_borrowed(self.borrowed())
    }

    /// The entries as plain values, each key with its hash. The dict cannot be changed while
    /// the iterator is alive.
    #[inline]
    pub fn entries_hashed(self) -> impl Iterator<Item = (Hashed<Value<'v>>, Value<'v>)> + 'v {
        let borrow = self.borrowed();
        // SAFETY: the closure below takes the borrow, so the iterator that reads the entries
        // has it for as long as it is alive.
        let entries = unsafe { content_while_borrowed(&borrow) }.iter_hashed();
        entries.map(move |(k, v)| {
            // The borrow lives as long as the iterator.
            let _borrow = &borrow;
            (k.copied(), *v)
        })
    }

    /// The keys, as plain values. The dict cannot be changed while the iterator is alive.
    #[inline]
    pub fn keys(self) -> impl ExactSizeIterator<Item = Value<'v>> + 'v {
        self.entries().map(|(k, _v)| k)
    }

    /// The values, as plain values. The dict cannot be changed while the iterator is alive.
    #[inline]
    pub fn values(self) -> impl ExactSizeIterator<Item = Value<'v>> + 'v {
        self.entries().map(|(_k, v)| v)
    }

    /// Removes every entry. An error if the dict is frozen or being iterated.
    #[inline]
    pub fn clear(self) -> crate::Result<()> {
        self.mutable()?.aref.clear();
        Ok(())
    }
}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> StarlarkDict<'v, K, V> {
    /// The immutable empty dict, which needs no heap.
    #[inline]
    pub fn empty() -> Self {
        Self::of_repr(DictRepr::Frozen(VALUE_EMPTY_FROZEN_DICT.at()))
    }

    /// The handle to this dict, for a field or a return.
    #[inline]
    pub fn handle(self) -> ValueOf<'v, Self> {
        // The entries were `K`s and `V`s when this was read, which is all a handle says.
        ValueOf::new_unchecked(self.to_value())
    }

    /// The entries. Each is checked to be a `K` and a `V` as it is read, since the dict may
    /// have been changed since it was read as a `dict[K, V]`; in a frozen dict nothing is
    /// checked and every entry is `Ok`. The dict cannot be changed while the iterator is
    /// alive.
    pub fn iter(self) -> DictEntries<'v, K, V> {
        DictEntries {
            frozen: self.repr.is_frozen(),
            entries: self.entries(),
            _marker: PhantomData,
        }
    }

    /// The entries, each converted by `K2`, a conversion from `K`, and `V2`, a conversion from
    /// `V`. Each is checked, as for [`iter`](Self::iter). The dict cannot be changed while
    /// the iterator is alive.
    pub fn try_items<K2, V2>(self) -> impl Iterator<Item = crate::Result<(K2, V2)>> + 'v
    where
        K2: UnpackValue<'v, Type = K>,
        V2: UnpackValue<'v, Type = V>,
    {
        self.entries()
            .map(|(k, v)| Ok((K2::unpack_value_err(k)?, V2::unpack_value_err(v)?)))
    }

    /// The value at `key`, checked to be a `V` unless the dict is frozen. An error also if
    /// `key` does not hash.
    pub fn lookup(self, key: Value<'v>) -> crate::Result<Option<ValueOf<'v, V>>> {
        self.get(key)?.map(|v| self.checked(v)).transpose()
    }

    /// The value at a string key, checked to be a `V` unless the dict is frozen.
    pub fn lookup_str(self, key: &str) -> crate::Result<Option<ValueOf<'v, V>>> {
        self.get_str(key).map(|v| self.checked(v)).transpose()
    }

    #[inline]
    fn checked(self, v: Value<'v>) -> crate::Result<ValueOf<'v, V>> {
        if self.repr.is_frozen() {
            Ok(ValueOf::new_unchecked(v))
        } else {
            ValueOf::new_err(v)
        }
    }

    /// Sets the value at `key`, and gives the value that was there. An error if the dict is
    /// frozen, if it is being iterated, or if `key` does not hash.
    #[inline]
    pub fn insert(
        self,
        key: ValueOf<'v, K>,
        value: ValueOf<'v, V>,
    ) -> crate::Result<Option<Value<'v>>> {
        let key = key.to_value().get_hashed()?;
        Ok(self.mutable()?.aref.insert_hashed(key, value.to_value()))
    }

    /// Removes the entry at `key`, and gives its value. An error if the dict is frozen, if it
    /// is being iterated, or if `key` does not hash.
    #[inline]
    pub fn remove(self, key: Value<'v>) -> crate::Result<Option<Value<'v>>> {
        let key = key.get_hashed()?;
        Ok(self.mutable()?.aref.remove_hashed(key))
    }
}

impl<'v, K, V> Debug for StarlarkDict<'v, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StarlarkDict").field(&self.repr).finish()
    }
}

impl<'v, K, V> Display for StarlarkDict<'v, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.repr.to_value(), f)
    }
}

/// The entries of a dict that is borrowed, for an iterator that keeps the borrow.
///
/// # Safety
///
/// The reference is not used after `borrow` is dropped.
#[inline]
unsafe fn content_while_borrowed<'v>(borrow: &DictRef<'v>) -> &'v SmallMap<Value<'v>, Value<'v>> {
    let content: *const SmallMap<Value<'v>, Value<'v>> = &borrow.content;
    // SAFETY: the dict is in the heap, which lives for `'v`, and it is not changed while it
    // is borrowed, which it is for as long as the reference is used.
    unsafe { &*content }
}

/// The entries of a dict as plain values: [`StarlarkDict::entries`].
///
/// While it is alive the dict is borrowed, so a change to the dict is the error it is in a
/// Starlark `for` loop.
pub struct DictValues<'v> {
    entries: small_map::Iter<'v, Value<'v>, Value<'v>>,
    /// Keeps the dict from being changed under `entries`.
    _borrow: DictRef<'v>,
}

impl<'v> DictValues<'v> {
    #[inline]
    fn of_borrowed(borrow: DictRef<'v>) -> Self {
        DictValues {
            // SAFETY: the iterator is stored with the borrow.
            entries: unsafe { content_while_borrowed(&borrow) }.iter(),
            _borrow: borrow,
        }
    }
}

impl<'v> Iterator for DictValues<'v> {
    type Item = (Value<'v>, Value<'v>);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.entries.next().map(|(k, v)| (*k, *v))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

impl<'v> ExactSizeIterator for DictValues<'v> {}

/// The entries of a `dict[K, V]`: [`StarlarkDict::iter`]. The dict cannot be changed while
/// this is alive.
pub struct DictEntries<'v, K, V> {
    frozen: bool,
    entries: DictValues<'v>,
    _marker: PhantomData<fn() -> (K, V)>,
}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> Iterator for DictEntries<'v, K, V> {
    type Item = crate::Result<(ValueOf<'v, K>, ValueOf<'v, V>)>;

    fn next(&mut self) -> Option<Self::Item> {
        let (k, v) = self.entries.next()?;
        Some(if self.frozen {
            Ok((ValueOf::new_unchecked(k), ValueOf::new_unchecked(v)))
        } else {
            (|| Ok((ValueOf::new_err(k)?, ValueOf::new_err(v)?)))()
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

/// The handle to a dict reads it as the dict does: these are [`StarlarkDict`]'s methods of
/// the same names, so that a field is read without a step in between.
impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> ValueOf<'v, StarlarkDict<'v, K, V>> {
    /// The empty dict.
    pub fn empty_dict() -> Self {
        Self::new_unchecked(Value::new_empty_dict())
    }

    /// The dict. Its entries are not checked here: what reads them as `K`s and `V`s checks
    /// each, as [`StarlarkDict::iter`] says. ([`try_get`](ValueOf::try_get) is this with
    /// every entry checked first.)
    #[inline]
    pub fn dict(self) -> StarlarkDict<'v, K, V> {
        StarlarkDict::of_repr(
            DictRepr::of_value(self.to_value()).expect("a value's type does not change"),
        )
    }

    /// [`StarlarkDict::len`].
    #[inline]
    pub fn len(self) -> usize {
        self.dict().len()
    }

    /// [`StarlarkDict::is_empty`].
    #[inline]
    pub fn is_empty(self) -> bool {
        self.dict().is_empty()
    }

    /// [`StarlarkDict::entries`].
    #[inline]
    pub fn entries(self) -> DictValues<'v> {
        self.dict().entries()
    }

    /// [`StarlarkDict::iter`].
    pub fn iter(self) -> DictEntries<'v, K, V> {
        self.dict().iter()
    }

    /// [`StarlarkDict::try_items`].
    pub fn try_items<K2, V2>(self) -> impl Iterator<Item = crate::Result<(K2, V2)>> + 'v
    where
        K2: UnpackValue<'v, Type = K>,
        V2: UnpackValue<'v, Type = V>,
    {
        self.dict().try_items()
    }

    /// [`StarlarkDict::get_str`].
    #[inline]
    pub fn get_str(self, key: &str) -> Option<Value<'v>> {
        self.dict().get_str(key)
    }

    /// [`StarlarkDict::lookup`].
    pub fn lookup(self, key: Value<'v>) -> crate::Result<Option<ValueOf<'v, V>>> {
        self.dict().lookup(key)
    }

    /// [`StarlarkDict::lookup_str`].
    pub fn lookup_str(self, key: &str) -> crate::Result<Option<ValueOf<'v, V>>> {
        self.dict().lookup_str(key)
    }
}

#[cfg(test)]
mod tests {
    use crate::values::Heap;
    use crate::values::StarlarkTypeExpr;
    use crate::values::UnpackValue;
    use crate::values::ValueOf;
    use crate::values::dict::AllocDict;
    use crate::values::dict::StarlarkDict;
    use crate::values::int::IntView;
    use crate::values::string::StarlarkStr;

    #[test]
    fn test_reading() {
        Heap::temp(|heap| {
            let dict = heap.alloc(AllocDict([("a", 1), ("b", 2)])).to_value();
            let (handle, reading) =
                ValueOf::<StarlarkDict<StarlarkStr, IntView>>::new_with_get(dict).unwrap();
            assert!(reading.to_value().ptr_eq(dict));
            assert!(reading.handle().to_value().ptr_eq(dict));
            assert_eq!(2, reading.len());
            assert_eq!(2, reading.iter().count());
            assert_eq!(2, reading.entries().count());
            assert_eq!(2, reading.keys().count());
            assert_eq!(2, reading.entries_hashed().count());
            assert!(reading.lookup_str("a").unwrap().is_some());
            assert!(reading.get_str("a").is_some());
            // The handle reads the dict as the dict does.
            assert_eq!(2, handle.len());
            assert_eq!(2, handle.iter().count());
            // A parameter spelled as the type is the reading.
            assert_eq!(2, StarlarkDict::from_value(dict).unwrap().len());
            assert!(<StarlarkDict<IntView, IntView>>::unpack_value_opt(dict).is_none());
        });
    }

    /// `StarlarkDict<'v>` is the type `dict` and `dict[typing.Any, typing.Any]`; they are one
    /// type.
    #[test]
    fn test_the_dict_type_and_a_dict_of_anything_are_one() {
        assert_eq!(
            <StarlarkDict as StarlarkTypeExpr>::starlark_type_repr(),
            <StarlarkDict as crate::values::StarlarkType>::get_type_starlark_repr(),
        );
        Heap::temp(|heap| {
            let dict = heap.alloc(AllocDict([("a", 1)])).to_value();
            let typed = <StarlarkDict<StarlarkStr, IntView>>::unpack_value_opt(dict).unwrap();
            assert!(typed.untyped().to_value().ptr_eq(dict));
            assert!(ValueOf::<StarlarkDict>::new(dict).is_some());
        });
    }

    /// A reading that is held does not keep the dict from changing and reads it as changed.
    /// An iterator does keep it, and the change is an error, not a panic.
    #[test]
    fn test_reading_and_changing() {
        Heap::temp(|heap| {
            let dict = heap.alloc(AllocDict([("a", 1)])).to_value();
            let reading = <StarlarkDict<StarlarkStr, IntView>>::unpack_value_opt(dict).unwrap();
            let b = ValueOf::alloc(heap, "b");
            let two = ValueOf::alloc(heap, 2);
            assert!(reading.insert(b, two).unwrap().is_none());
            assert_eq!(2, reading.len());
            {
                let mut entries = reading.entries();
                assert!(entries.next().is_some());
                let e = reading.insert(b, two).err().unwrap();
                assert!(e.to_string().contains("mutates an iterable"), "{e}");
                let e = reading.clear().err().unwrap();
                assert!(e.to_string().contains("mutates an iterable"), "{e}");
                assert_eq!(1, entries.len());
            }
            assert!(reading.remove(b.to_value()).unwrap().is_some());
            assert_eq!(1, reading.len());
            // An entry that is no longer a `K` and a `V` is an error where it is read.
            reading
                .untyped()
                .insert(
                    ValueOf::new_unchecked(heap.alloc(1i32).to_value()),
                    ValueOf::new_unchecked(dict),
                )
                .unwrap();
            assert!(reading.iter().last().unwrap().is_err());
            reading.clear().unwrap();
            assert!(reading.is_empty());
        });
    }
}
