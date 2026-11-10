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

use std::any::TypeId;
use std::cell::Ref;
use std::cell::RefCell;
use std::cell::RefMut;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;
use std::hash::Hasher;
use std::mem;
use std::ops::Deref;

use allocative::Allocative;
use display_container::fmt_keyed_container;
use either::Either;
use serde::Serialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::StarlarkRepresentations;
use starlark_derive::starlark_value;
use starlark_map::Equivalent;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::cast::transmute;
use crate::collections::Hashed;
use crate::collections::SmallMap;
use crate::environment::Methods;
use crate::hint::unlikely;
use crate::static_starlark_value;
use crate::typing::Ty;
use crate::util::refcell::unleak_borrow;
use crate::values::AllocFrozenRepresentation;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenAllocator;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::StaticFreezePlan;
use crate::values::StringValue;
use crate::values::Trace;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::comparison::equals_small_map;
use crate::values::dict::DictRef;
use crate::values::error::ValueError;
use crate::values::string::str_type::hash_string_value;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::types::dict::dict_type::DictType;

#[derive(
    Clone,
    Default,
    Trace,
    Debug,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
pub(crate) struct DictGen<T>(pub(crate) T);

impl<'v, T: DictLike<'v>> Display for DictGen<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_keyed_container(f, "{", "}", ": ", self.0.content().iter())
    }
}

impl<'v> Display for Dict<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_keyed_container(f, "{", "}", ": ", self.iter())
    }
}

/// Define the dict type.
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
pub struct Dict<'v> {
    /// The data stored by the dictionary. The keys must all be hashable values.
    pub(crate) content: SmallMap<Value<'v>, Value<'v>>,
}

impl<'v> StarlarkTypeRepr for Dict<'v> {
    type Canonical = <DictType<Value<'static>, Value<'static>> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        Self::Canonical::starlark_type_repr()
    }
}

/// A frozen dict is a `Dict` without the `RefCell`: the two variants differ only in whether
/// mutation is possible, and nothing ever hands out a `&mut` to the frozen one.
///
/// Alias is used in `StarlarkDocs` derive.
pub(crate) type FrozenDict = DictGen<Dict<'static>>;

pub(crate) type MutableDict<'v> = DictGen<RefCell<Dict<'v>>>;

crate::register_simple_vtable_entry!(FrozenDict);
// SAFETY: The vtable entry is registered above. The deser type id is
// lifetime-erased, so the `'static` instantiation covers all heap lifetimes.
unsafe impl<'v> crate::__derive_refs::VtableRegistered for DictGen<Dict<'v>> {}

static_starlark_value!(
    pub(crate) VALUE_EMPTY_FROZEN_DICT: DictGen<Dict<'static>> = DictGen(Dict {
        content: SmallMap::new(),
    })
);

/// Every dict is a `dict[typing.Any, typing.Any]`.
impl<'v> From<ValueOf<'v, DictView<'v>>> for ValueOf<'v, DictType<Value<'v>, Value<'v>>> {
    #[inline]
    fn from(value: ValueOf<'v, DictView<'v>>) -> Self {
        Self::new_unchecked(value.to_value())
    }
}

impl<'v> AllocValue<'v> for Dict<'v> {
    type Type = DictType<Value<'v>, Value<'v>>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::alloc(heap, DictGen(RefCell::new(self))).into()
    }
}

impl<'v> Heap<'v> {
    /// Allocate a dictionary with pre-allocated and pre-hashed keys and values.
    pub fn alloc_dict(self, content: SmallMap<Value<'v>, Value<'v>>) -> Value<'v> {
        self.alloc(Dict::new(content))
    }
}

impl<'fv> AllocFrozenValue<'fv> for Dict<'fv> {
    type Type = DictType<Value<'fv>, Value<'fv>>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        ValueOf::alloc_frozen(heap, DictGen(self)).into()
    }
}

impl<'v> Dict<'v> {
    pub(crate) fn is_dict_type(x: TypeId) -> bool {
        x == TypeId::of::<DictGen<Dict<'static>>>()
            || x == TypeId::of::<DictGen<RefCell<Dict<'static>>>>()
    }

    pub(crate) unsafe fn from_value_unchecked_mut(x: Value<'v>) -> RefMut<'v, Self> {
        unsafe {
            let dict = &x.downcast_ref_unchecked::<DictGen<RefCell<Dict<'v>>>>().0;
            dict.borrow_mut()
        }
    }
}

/// Helper type for lookups, not useful.
#[derive(Eq, PartialEq)]
pub struct ValueStr<'a>(pub(crate) &'a str);

impl<'a> Hash for ValueStr<'a> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_string_value(self.0, state)
    }
}

impl<'v> Equivalent<Value<'v>> for ValueStr<'_> {
    fn equivalent(&self, key: &Value<'v>) -> bool {
        key.unpack_str() == Some(self.0)
    }
}

impl<'v> Dict<'v> {
    /// The result of calling `type()` on dictionaries.
    pub const TYPE: &'static str = "dict";

    /// Dict type string as Starlark frozen string value.
    pub fn get_type_value_static() -> StringValue<'static> {
        DictGen::<Dict>::get_type_value_static()
    }

    /// This function is deprecated.
    /// Use [`AllocDict`](crate::values::dict::AllocDict) or [`SmallMap`]
    /// to allocate a new dictionary on the heap.
    pub fn new(content: SmallMap<Value<'v>, Value<'v>>) -> Self {
        Self { content }
    }

    /// Number of elements in the dict.
    pub fn len(&self) -> usize {
        self.content.len()
    }

    /// Is the dict empty?
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Iterate through the key/value pairs in the dictionary.
    pub fn iter<'a>(&'a self) -> impl ExactSizeIterator<Item = (Value<'v>, Value<'v>)> + 'a {
        self.content.iter().map(|(l, r)| (*l, *r))
    }

    /// Iterate through the key/value pairs in the dictionary, but retaining the hash of the keys.
    pub fn iter_hashed<'a>(&'a self) -> impl Iterator<Item = (Hashed<Value<'v>>, Value<'v>)> + 'a
    where
        'v: 'a,
    {
        self.content.iter_hashed().map(|(l, r)| (l.copied(), *r))
    }

    /// Iterator over keys.
    pub fn keys<'a>(&'a self) -> impl Iterator<Item = Value<'v>> + 'a {
        self.content.keys().copied()
    }

    /// Iterator over keys.
    pub fn values<'a>(&'a self) -> impl Iterator<Item = Value<'v>> + 'a {
        self.content.values().copied()
    }

    /// Get the value associated with a particular key. Will be [`Err`] if the key is not hashable,
    /// and otherwise [`Some`] if the key exists in the dictionary and [`None`] otherwise.
    pub fn get(&self, key: Value<'v>) -> crate::Result<Option<Value<'v>>> {
        Ok(self.get_hashed(key.get_hashed()?))
    }

    /// Lookup the value by the given prehashed key.
    pub fn get_hashed(&self, key: Hashed<Value<'v>>) -> Option<Value<'v>> {
        self.content.get_hashed_by_value(key).copied()
    }

    /// Get the value associated with a particular string. Equivalent to allocating the
    /// string on the heap, turning it into a value, and looking up using that.
    pub fn get_str(&self, key: &str) -> Option<Value<'v>> {
        self.content.get(&ValueStr(key)).copied()
    }

    /// Like [`Dict::get_str`], but where you already have the hash.
    pub fn get_str_hashed(&self, key: Hashed<&str>) -> Option<Value<'v>> {
        self.content
            .get_hashed_by_value(Hashed::new_unchecked(key.hash(), ValueStr(key.key())))
            .copied()
    }

    /// The content as a map keyed by strings, if every key is one.
    ///
    /// A `SmallMap<StringValue<'v>, Value<'v>>` and a `SmallMap<Value<'v>, Value<'v>>` whose keys
    /// are all strings are the same map: `StringValue` is a `#[repr(transparent)]` wrapper of
    /// `Value`, and its `Hash` and `Eq` agree with the `Value`'s on a string. This and
    /// [`from_string_keyed`](Dict::from_string_keyed) convert between the two without touching
    /// the entries.
    pub(crate) fn downcast_ref_key_string(&self) -> Option<&SmallMap<StringValue<'v>, Value<'v>>> {
        for &key in self.content.keys() {
            if unlikely(!key.is_str()) {
                return None;
            }
        }

        // SAFETY: All keys are strings (just checked), and the two map types have one layout and
        // the same key hashing and equality, see above.
        Some(unsafe {
            transmute!(&SmallMap<Value, Value>, &SmallMap<StringValue, Value>, &self.content)
        })
    }

    /// A dict from a map keyed by strings, without rehashing; the inverse of
    /// [`downcast_ref_key_string`](Dict::downcast_ref_key_string).
    pub(crate) fn from_string_keyed(content: SmallMap<StringValue<'v>, Value<'v>>) -> Dict<'v> {
        // SAFETY: The two map types have one layout and the same key hashing and equality, see
        // `downcast_ref_key_string`.
        let content = unsafe {
            mem::transmute::<SmallMap<StringValue<'v>, Value<'v>>, SmallMap<Value<'v>, Value<'v>>>(
                content,
            )
        };
        Dict { content }
    }

    /// Reserve capacity to insert `additional` elements without reallocating.
    pub fn reserve(&mut self, additional: usize) {
        self.content.reserve(additional);
    }

    /// Insert a key/value pair into the dictionary.
    pub fn insert_hashed(&mut self, key: Hashed<Value<'v>>, value: Value<'v>) -> Option<Value<'v>> {
        self.content.insert_hashed(key, value)
    }

    /// Remove given key from the dictionary.
    pub fn remove_hashed(&mut self, key: Hashed<Value<'v>>) -> Option<Value<'v>> {
        self.content.shift_remove_hashed(key.as_ref())
    }

    /// Remove all elements from the dictionary.
    pub fn clear(&mut self) {
        self.content.clear();
    }
}

/// The direct freeze plan reusing the static empty dict.
///
/// A free function: naming the static's rebrand inside the early-bound
/// `prepare_freeze` trait method trips a spurious rustc bound failure; the
/// identical expression resolves here.
fn empty_dict_plan<'v, 'fv>() -> StaticFreezePlan<'v, 'fv, DictGen<RefCell<Dict<'v>>>> {
    StaticFreezePlan::direct(VALUE_EMPTY_FROZEN_DICT.at())
}

impl<'v> Freeze<'v> for DictGen<RefCell<Dict<'v>>> {
    type Frozen<'fv> = DictGen<Dict<'fv>>;

    fn prepare_freeze<'fv>(
        &self,
        _freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<StaticFreezePlan<'v, 'fv, Self>>
    where
        Self::Frozen<'fv>: StarlarkValue<'fv>,
    {
        if self.0.content().is_empty() {
            Ok(empty_dict_plan())
        } else {
            Ok(StaticFreezePlan::allocate())
        }
    }

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let entries = self.0.into_inner().content;
        let mut content = SmallMap::with_capacity(entries.len());
        for (key, value) in entries.into_iter_hashed() {
            // Freezing does not change the hash.
            let key = Hashed::new_unchecked(key.hash(), freezer.freeze(key.into_key())?);
            content.insert_hashed_unique_unchecked(key, freezer.freeze(value)?);
        }
        Ok(DictGen(Dict { content }))
    }
}

trait DictLike<'v>: Debug + Allocative {
    type ContentRef<'a>: Deref<Target = SmallMap<Value<'v>, Value<'v>>>
    where
        Self: 'a,
        'v: 'a;
    fn content<'a>(&'a self) -> Self::ContentRef<'a>;
    // These functions are unsafe for the same reason
    // `StarlarkValue` iterator functions are unsafe.
    unsafe fn iter_start(&self) -> crate::Result<()>;
    unsafe fn content_unchecked(&self) -> &SmallMap<Value<'v>, Value<'v>>;
    unsafe fn iter_stop(&self);
    fn set_at(&self, index: Hashed<Value<'v>>, value: Value<'v>) -> crate::Result<()>;
}

impl<'v> DictLike<'v> for RefCell<Dict<'v>> {
    type ContentRef<'a>
        = Ref<'a, SmallMap<Value<'v>, Value<'v>>>
    where
        Self: 'a,
        'v: 'a;

    fn content<'a>(&'a self) -> Ref<'a, SmallMap<Value<'v>, Value<'v>>> {
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
    unsafe fn content_unchecked(&self) -> &SmallMap<Value<'v>, Value<'v>> {
        unsafe {
            // SAFETY: this function contract is, caller must ensure that the value is borrowed.
            &self.try_borrow_unguarded().ok().unwrap_unchecked().content
        }
    }

    fn set_at(&self, index: Hashed<Value<'v>>, alloc_value: Value<'v>) -> crate::Result<()> {
        match self.try_borrow_mut() {
            Ok(mut xs) => {
                xs.content.insert_hashed(index, alloc_value);
                Ok(())
            }
            Err(_) => Err(crate::Error::new_other(ValueError::MutationDuringIteration)),
        }
    }
}

/// The frozen half of the pair: `DictGen<Dict<'v>>`. Immutability is structural — there is no
/// `RefCell`, so no caller can obtain the `&mut` the mutators need.
impl<'v> DictLike<'v> for Dict<'v> {
    type ContentRef<'a>
        = &'a SmallMap<Value<'v>, Value<'v>>
    where
        Self: 'a,
        'v: 'a;

    fn content<'a>(&'a self) -> &'a SmallMap<Value<'v>, Value<'v>> {
        &self.content
    }

    unsafe fn iter_start(&self) -> crate::Result<()> {
        Ok(())
    }

    unsafe fn iter_stop(&self) {}

    unsafe fn content_unchecked(&self) -> &SmallMap<Value<'v>, Value<'v>> {
        &self.content
    }

    fn set_at(&self, _index: Hashed<Value<'v>>, _value: Value<'v>) -> crate::Result<()> {
        Err(crate::Error::new_other(
            ValueError::CannotMutateImmutableValue,
        ))
    }
}

starlark::methods_static!(DICT_METHODS = crate::values::types::dict::methods::dict_methods);

/// A `dict` in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub(crate) enum DictView<'v> {
    Frozen(ValueTyped<'v, DictGen<Dict<'v>>>),
    Unfrozen(ValueTyped<'v, DictGen<RefCell<Dict<'v>>>>),
}

/// `$e` with `$x` bound to the representation's `DictLike`.
macro_rules! with_dict {
    ($self:expr, |$x:ident| $e:expr) => {
        match $self {
            DictView::Frozen($x) => {
                let $x = &$x.as_ref().0;
                $e
            }
            DictView::Unfrozen($x) => {
                let $x = &$x.as_ref().0;
                $e
            }
        }
    };
}

impl<'v> DictView<'v> {
    #[inline]
    fn content(
        self,
    ) -> Either<Ref<'v, SmallMap<Value<'v>, Value<'v>>>, &'v SmallMap<Value<'v>, Value<'v>>> {
        match self {
            DictView::Frozen(x) => Either::Right(x.as_ref().0.content()),
            DictView::Unfrozen(x) => Either::Left(x.as_ref().0.content()),
        }
    }
}

#[starlark_value(type = Dict::TYPE)]
impl<'v> StarlarkType<'v> for DictView<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(DICT_METHODS.methods())
    }

    fn collect_repr(&self, r: &mut String) {
        // Fast path as repr() for dicts is quite hot
        r.push('{');
        for (i, (name, value)) in self.content().iter().enumerate() {
            if i != 0 {
                r.push_str(", ");
            }
            name.collect_repr(r);
            r.push_str(": ");
            value.collect_repr(r);
        }
        r.push('}');
    }

    fn collect_repr_cycle(&self, collector: &mut String) {
        collector.push_str("{...}");
    }

    fn to_bool(&self) -> bool {
        !self.content().is_empty()
    }

    fn equals(&self, other: Value<'v>) -> crate::Result<bool> {
        match DictRef::from_value(other) {
            None => Ok(false),
            Some(other) => equals_small_map(&self.content(), &other.content, |x, y| x.equals(*y)),
        }
    }

    fn at(&self, index: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match self.content().get_hashed_by_value(index.get_hashed()?) {
            Some(v) => Ok(v.to_value()),
            None => Err(crate::Error::new_other(ValueError::KeyNotFound(
                index.to_repr(),
            ))),
        }
    }

    fn length(&self) -> crate::Result<i32> {
        Ok(self.content().len() as i32)
    }

    fn is_in(&self, other: Value<'v>) -> crate::Result<bool> {
        Ok(self
            .content()
            .contains_key_hashed_by_value(other.get_hashed()?))
    }

    unsafe fn iterate(&self, me: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        unsafe {
            with_dict!(self, |d| d.iter_start())?;
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
        unsafe { with_dict!(self, |d| d.content_unchecked().keys().nth(index).copied()) }
    }

    unsafe fn iter_stop(&self) {
        unsafe { with_dict!(self, |d| d.iter_stop()) }
    }

    fn set_at(&self, index: Value<'v>, alloc_value: Value<'v>) -> crate::Result<()> {
        let index = index.get_hashed()?;
        with_dict!(self, |d| d.set_at(index, alloc_value))
    }

    fn bit_or(&self, rhs: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        let rhs = DictRef::from_value(rhs)
            .map_or_else(|| ValueError::unsupported_with(self, "|", rhs), Ok)?;
        let content = self.content();
        if content.is_empty() {
            return Ok(heap.alloc((*rhs).clone()));
        }
        // Might be faster if we preallocate the capacity, but then copying in the LHS
        // is more expensive and might oversize given the behaviour on duplicates.
        // If this becomes a bottleneck, benchmark.
        let mut items = content.clone();
        for (k, v) in rhs.iter_hashed() {
            items.insert_hashed(k, v);
        }
        Ok(heap.alloc(Dict::new(items)))
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(Ty::any_dict())
    }

    fn get_type_starlark_repr() -> Ty {
        Ty::any_dict()
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for DictGen<RefCell<Dict<'v>>> {
    type Type = DictView<'v>;
}

/// The empty frozen dict is a static.
impl<'fv> AllocFrozenRepresentation<'fv> for DictGen<Dict<'fv>> {
    type Repr = Self;

    fn alloc_frozen(self, allocator: FrozenAllocator<'fv>) -> ValueTyped<'fv, Self> {
        if self.0.content.is_empty() {
            VALUE_EMPTY_FROZEN_DICT.at()
        } else {
            allocator.alloc_simple(self)
        }
    }
}

#[starlark_value(alloc = by_hand)]
impl<'v> StarlarkValue<'v> for DictGen<Dict<'v>> {
    type Type = DictView<'v>;
}

impl<'v, T: DictLike<'v>> Serialize for DictGen<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_map(self.0.content().iter())
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;
    use crate::collections::SmallMap;
    use crate::values::Heap;
    use crate::values::dict::Dict;

    #[test]
    fn test_mutate_dict() {
        assert::is_true(
            r#"
x = {1: 2, 2: 4}
b1 = str(x) == "{1: 2, 2: 4}"
x[2] = 3
b2 = str(x) == "{1: 2, 2: 3}"
x[(3,4)] = 5
b3 = str(x) == "{1: 2, 2: 3, (3, 4): 5}"
b1 and b2 and b3
"#,
        );
    }

    #[test]
    fn test_get_str() -> crate::Result<()> {
        Heap::temp(|heap| {
            let k1 = heap.alloc_str("hello").get_hashed_value();
            let k2 = heap.alloc_str("world").get_hashed_value();
            let mut sm = SmallMap::new();
            sm.insert_hashed(k1, heap.alloc(12));
            sm.insert_hashed(k2, heap.alloc(56));
            let d = Dict::new(sm);

            assert_eq!(d.get(heap.alloc("hello"))?.unwrap().unpack_i32(), Some(12));
            assert_eq!(d.get(heap.alloc("foo"))?, None);
            assert_eq!(d.get_str("hello").unwrap().unpack_i32(), Some(12));
            assert_eq!(d.get_str("foo"), None);
            Ok(())
        })
    }

    #[test]
    fn test_repr_cycle() {
        assert::eq("d = {}; d[17] = d; repr(d)", "'{17: {...}}'");
        assert::eq("d = {}; d[17] = d; str(d)", "'{17: {...}}'");
    }
}
