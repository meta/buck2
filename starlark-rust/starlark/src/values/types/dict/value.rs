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
use std::cell::BorrowMutError;
use std::cell::Cell;
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
use dupe::Dupe;
use either::Either;
use serde::Serialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_value;
use starlark_map::Equivalent;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::cast::transmute;
use crate::collections::Hashed;
use crate::collections::SmallMap;
use crate::eval::Arguments;
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
use crate::values::dict::DictMut;
use crate::values::dict::DictRef;
use crate::values::dict::StarlarkDict;
use crate::values::error::ValueError;
use crate::values::function::SpecialBuiltinFunction;
use crate::values::string::str_type::hash_string_value;
use crate::values::typing::type_compiled::matchers::DictMatcherId;

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

/// A frozen dict is a `Dict` without the `DictCell`: the two variants differ only in whether
/// mutation is possible, and nothing ever hands out a `&mut` to the frozen one.
///
/// Alias is used in `StarlarkDocs` derive.
pub(crate) type FrozenDict = DictGen<Dict<'static>>;

pub(crate) type MutableDict<'v> = DictGen<DictCell<'v>>;

/// A mutable dict, and how much of it a `dict[K, V]` matcher is known to accept.
#[derive(Clone, Default, Trace, ProvidesStaticType, Allocative)]
pub(crate) struct DictCell<'v> {
    dict: RefCell<Dict<'v>>,
    /// A matcher and the number of leading entries it accepted, none of which has been replaced
    /// or removed since: entries are only ever added at the end.
    #[trace(unsafe_ignore)]
    #[allocative(skip)]
    checked: Cell<Option<(DictMatcherId, u32)>>,
}

// One word, because every mutable dict carries it.
const _: () = assert!(mem::size_of::<DictCell>() == mem::size_of::<RefCell<Dict>>() + 8);

/// The dict alone: what was checked depends on what else the process has run.
impl Debug for DictCell<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.dict, f)
    }
}

/// Testing a dict shorter than this costs less than remembering that it was tested.
const REMEMBER_FROM: usize = 16;

impl<'v> DictCell<'v> {
    fn new(dict: Dict<'v>) -> DictCell<'v> {
        DictCell {
            dict: RefCell::new(dict),
            checked: Cell::new(None),
        }
    }

    #[inline]
    pub(crate) fn borrow(&self) -> Ref<'_, Dict<'v>> {
        self.dict.borrow()
    }

    /// A caller may do anything to the dict, so what was checked is forgotten.
    #[inline]
    pub(crate) fn try_borrow_mut(&self) -> Result<RefMut<'_, Dict<'v>>, BorrowMutError> {
        let dict = self.dict.try_borrow_mut()?;
        self.checked.set(None);
        Ok(dict)
    }

    /// Whether every entry passes `entry_matches`, the test of the matcher `by` names. Entries
    /// the matcher has accepted before are not tested again; `by` must name no matcher whose
    /// answer for an entry can change.
    #[inline]
    fn all_match(
        &self,
        by: impl FnOnce() -> Option<DictMatcherId>,
        entry_matches: impl Fn(Value<'v>, Value<'v>) -> bool,
    ) -> bool {
        let dict = self.dict.borrow();
        let len = dict.content.len();
        let by = if len < REMEMBER_FROM { None } else { by() };
        let Some(by) = by else {
            return dict.iter().all(|(k, v)| entry_matches(k, v));
        };
        let checked = match self.checked.get() {
            Some((checked_by, checked)) if checked_by == by && checked as usize <= len => {
                checked as usize
            }
            _ => 0,
        };
        let matches = dict.iter().skip(checked).all(|(k, v)| entry_matches(k, v));
        if matches {
            // A dict too long for the count is one this cannot remember.
            self.checked
                .set(u32::try_from(len).ok().map(|len| (by, len)));
        }
        matches
    }
}

/// Whether `value` is a dict whose every entry passes `entry_matches`, the test of a
/// `dict[K, V]` matcher. `matcher` names it if its answer for an entry can never change; a
/// mutable dict then remembers the entries it accepted.
#[inline]
pub(crate) fn dict_entries_all_match<'v>(
    value: Value<'v>,
    matcher: impl FnOnce() -> Option<DictMatcherId>,
    entry_matches: impl Fn(Value<'v>, Value<'v>) -> bool,
) -> bool {
    if let Some(dict) = value.downcast_ref::<DictGen<Dict<'v>>>() {
        return dict.0.iter().all(|(k, v)| entry_matches(k, v));
    }
    match value.downcast_ref::<MutableDict<'v>>() {
        Some(dict) => dict.0.all_match(matcher, entry_matches),
        None => false,
    }
}

crate::register_simple_vtable_entry!(FrozenDict);
// SAFETY: The vtable entry is registered above. The deser type id is
// lifetime-erased, so the `'static` instantiation covers all heap lifetimes.
unsafe impl<'v> crate::__derive_refs::VtableRegistered for DictGen<Dict<'v>> {}

static_starlark_value!(
    pub(crate) VALUE_EMPTY_FROZEN_DICT: DictGen<Dict<'static>> = DictGen(Dict {
        content: SmallMap::new(),
    })
);

impl<'v> AllocValue<'v> for Dict<'v> {
    type Type = StarlarkDict<'v>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::alloc(heap, DictGen(DictCell::new(self)))
    }
}

impl<'fv> AllocFrozenValue<'fv> for Dict<'fv> {
    type Type = StarlarkDict<'fv>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        ValueOf::alloc_frozen(heap, DictGen(self))
    }
}

impl<'v> Dict<'v> {
    pub(crate) fn is_dict_type(x: TypeId) -> bool {
        x == TypeId::of::<DictGen<Dict<'static>>>()
            || x == TypeId::of::<DictGen<DictCell<'static>>>()
    }

    pub(crate) unsafe fn from_value_unchecked_mut(x: Value<'v>) -> RefMut<'v, Self> {
        unsafe {
            let dict = &x.downcast_ref_unchecked::<DictGen<DictCell<'v>>>().0;
            dict.checked.set(None);
            dict.dict.borrow_mut()
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
    /// Use [`AllocDict`](crate::values::dict::AllocDict)
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
fn empty_dict_plan<'v, 'fv>() -> StaticFreezePlan<'v, 'fv, DictGen<DictCell<'v>>> {
    StaticFreezePlan::direct(VALUE_EMPTY_FROZEN_DICT.at())
}

impl<'v> Freeze<'v> for DictGen<DictCell<'v>> {
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
        let entries = self.0.dict.into_inner().content;
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

impl<'v> DictLike<'v> for DictCell<'v> {
    type ContentRef<'a>
        = Ref<'a, SmallMap<Value<'v>, Value<'v>>>
    where
        Self: 'a,
        'v: 'a;

    fn content<'a>(&'a self) -> Ref<'a, SmallMap<Value<'v>, Value<'v>>> {
        Ref::map(self.dict.borrow(), |x| &x.content)
    }

    #[inline]
    unsafe fn iter_start(&self) -> crate::Result<()> {
        // A method may hold this value mutably borrowed while it iterates one of its
        // arguments, e.g. `d.update([d])`; that is an error, not a panic.
        match self.dict.try_borrow() {
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
            unleak_borrow(&self.dict);
        }
    }

    #[inline]
    unsafe fn content_unchecked(&self) -> &SmallMap<Value<'v>, Value<'v>> {
        unsafe {
            // SAFETY: this function contract is, caller must ensure that the value is borrowed.
            &self
                .dict
                .try_borrow_unguarded()
                .ok()
                .unwrap_unchecked()
                .content
        }
    }

    fn set_at(&self, index: Hashed<Value<'v>>, alloc_value: Value<'v>) -> crate::Result<()> {
        match self.dict.try_borrow_mut() {
            Ok(mut xs) => {
                // A new key goes to the end and leaves what was checked as it was.
                if xs.content.insert_hashed(index, alloc_value).is_some() {
                    self.checked.set(None);
                }
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

/// Which of its representations a `dict` is.
#[derive(Debug, Clone, Copy, Dupe)]
pub(crate) enum DictRepr<'v> {
    Frozen(ValueTyped<'v, DictGen<Dict<'v>>>),
    Unfrozen(ValueTyped<'v, DictGen<DictCell<'v>>>),
}

/// `$e` with `$x` bound to the representation's `DictLike`.
macro_rules! with_dict {
    ($self:expr, |$x:ident| $e:expr) => {
        match $self {
            DictRepr::Frozen($x) => {
                let $x = &$x.as_ref().0;
                $e
            }
            DictRepr::Unfrozen($x) => {
                let $x = &$x.as_ref().0;
                $e
            }
        }
    };
}

impl<'v> DictRepr<'v> {
    /// `value` as a dict, if it is one. The representation is told by whether the value is
    /// frozen, so one representation is tested, not both in turn.
    #[inline(always)] // A step of a conversion: see `unpack_then`.
    pub(crate) fn of_value(value: Value<'v>) -> Option<Self> {
        if value.is_frozen() {
            ValueTyped::new(value).map(DictRepr::Frozen)
        } else {
            ValueTyped::new(value).map(DictRepr::Unfrozen)
        }
    }

    #[inline]
    pub(crate) fn to_value(self) -> Value<'v> {
        match self {
            DictRepr::Frozen(x) => x.to_value(),
            DictRepr::Unfrozen(x) => x.to_value(),
        }
    }

    #[inline]
    pub(crate) fn is_frozen(self) -> bool {
        matches!(self, DictRepr::Frozen(_))
    }

    /// The dict, borrowed for as long as the reference lives if it is mutable.
    #[inline]
    pub(crate) fn to_ref(self) -> DictRef<'v> {
        DictRef {
            aref: match self {
                DictRepr::Frozen(x) => Either::Right(&x.as_ref().0),
                DictRepr::Unfrozen(x) => Either::Left(x.as_ref().0.borrow()),
            },
        }
    }

    /// The dict, to change it: an error if it is frozen or being iterated.
    #[inline]
    pub(crate) fn to_mut(self) -> crate::Result<DictMut<'v>> {
        match self {
            DictRepr::Unfrozen(x) => match x.as_ref().0.try_borrow_mut() {
                Ok(aref) => Ok(DictMut { aref }),
                Err(_) => Err(cannot_mutate(ValueError::MutationDuringIteration)),
            },
            DictRepr::Frozen(_) => Err(cannot_mutate(ValueError::CannotMutateImmutableValue)),
        }
    }

    #[inline]
    fn content(
        self,
    ) -> Either<Ref<'v, SmallMap<Value<'v>, Value<'v>>>, &'v SmallMap<Value<'v>, Value<'v>>> {
        match self {
            DictRepr::Frozen(x) => Either::Right(x.as_ref().0.content()),
            DictRepr::Unfrozen(x) => Either::Left(x.as_ref().0.content()),
        }
    }
}

#[cold]
#[inline(never)]
fn cannot_mutate(error: ValueError) -> crate::Error {
    crate::Error::new_other(error)
}

fn unpack_pair<'v>(pair: Value<'v>, heap: Heap<'v>) -> crate::Result<(Value<'v>, Value<'v>)> {
    let mut it = pair.iterate(heap)?;
    if let Some(first) = it.next() {
        if let Some(second) = it.next() {
            if it.next().is_none() {
                return Ok((first, second));
            }
        }
    }
    Err(anyhow::anyhow!(
        "Found a non-pair element in the positional argument of dict(): {}",
        pair.to_repr(),
    )
    .into())
}

#[starlark_value(type = Dict::TYPE, type_expr = by_hand, methods = crate::values::types::dict::methods::dict_methods)]
impl<'v> StarlarkType<'v> for StarlarkDict<'v> {
    /// [dict](
    /// https://github.com/bazelbuild/starlark/blob/master/spec.md#dict
    /// ): creates a dictionary.
    ///
    /// `dict` creates a dictionary. It accepts up to one positional argument,
    /// which is interpreted as an iterable of two-element sequences
    /// (pairs), each specifying a key/value pair in the
    /// resulting dictionary.
    ///
    /// `dict` also accepts any number of keyword arguments, each of which
    /// specifies a key/value pair in the resulting dictionary; each keyword
    /// is treated as a string.
    ///
    /// ```
    /// # starlark::assert::all_true(r#"
    /// dict() == {}
    /// dict(**{'a': 1}) == {'a': 1}
    /// dict({'a': 1}) == {'a': 1}
    /// dict([(1, 2), (3, 4)]) == {1: 2, 3: 4}
    /// dict([(1, 2), ['a', 'b']]) == {1: 2, 'a': 'b'}
    /// dict(one=1, two=2) == {'one': 1, 'two': 2}
    /// dict([(1, 2)], x=3) == {1: 2, 'x': 3}
    /// dict([('x', 2)], x=3) == {'x': 3}
    /// # "#);
    /// # starlark::assert::is_true(r#"
    /// x = {'a': 1}
    /// y = dict([('x', 2)], **x)
    /// x == {'a': 1} and y == {'x': 2, 'a': 1}
    /// # "#);
    /// ```
    #[starlark(
        constructor,
        speculative_exec_safe,
        special_builtin_function = SpecialBuiltinFunction::Dict,
    )]
    fn dict(args: &Arguments<'v, '_>, heap: Heap<'v>) -> starlark::Result<Dict<'v>> {
        // Dict is super hot, and has a slightly odd signature, so we can do a bunch of special cases on it.
        // In particular, we don't generate the kwargs if there are no positional arguments.
        // Therefore we make it take the raw Arguments.
        // It might have one positional argument, which could be a dict or an array of pairs.
        // It might have named/kwargs arguments, which we copy over (afterwards).

        let pos = args.optional1(heap)?;
        let kwargs = args.names()?;

        match pos {
            None => Ok(kwargs),
            Some(pos) => {
                let mut result: Dict = match DictRef::from_value(pos) {
                    Some(pos) => {
                        let mut result = (*pos).clone();
                        result.reserve(kwargs.len());
                        result
                    }
                    None => {
                        let it = pos.iterate(heap)?;
                        let mut result = SmallMap::with_capacity(it.size_hint().0 + kwargs.len());
                        for el in it {
                            let (k, v) = unpack_pair(el, heap)?;
                            let k = k.get_hashed()?;
                            result.insert_hashed(k, v);
                        }
                        Dict::new(result)
                    }
                };
                for (k, v) in kwargs.iter_hashed() {
                    result.insert_hashed(k, v);
                }
                Ok(result)
            }
        }
    }

    fn collect_repr(this: Self, r: &mut String) {
        // Fast path as repr() for dicts is quite hot
        r.push('{');
        for (i, (name, value)) in this.repr().content().iter().enumerate() {
            if i != 0 {
                r.push_str(", ");
            }
            name.collect_repr(r);
            r.push_str(": ");
            value.collect_repr(r);
        }
        r.push('}');
    }

    fn collect_repr_cycle(_this: Self, collector: &mut String) {
        collector.push_str("{...}");
    }

    fn to_bool(this: Self) -> bool {
        !this.repr().content().is_empty()
    }

    // The operand is a plain value read with `from_value`, not a typed `Self`: read as a
    // `dict[typing.Any, typing.Any]` it is borrowed once more, to look at entries that there
    // is nothing to check about. Inlined into each representation's entry, as the macro does
    // by itself for a typed method.
    #[inline(always)]
    fn equals(this: Self, other: Value<'v>) -> crate::Result<bool> {
        match StarlarkDict::from_value(other) {
            Some(other) => {
                equals_small_map(&this.repr().content(), &other.repr().content(), |x, y| {
                    x.equals(*y)
                })
            }
            None => Ok(false),
        }
    }

    // `at` and `is_in` take a `Value`, so they are not typed methods, which the macro inlines
    // into each representation's vtable entry by itself; and they are too big for the hint.
    // Out of line they are one copy that dispatches on the representation, behind an entry per
    // representation that only says which it is: 8 more instructions on `d[k]` and 17 more on
    // `k in d`, with the `starlark` crate built as one codegen unit.
    #[inline(always)]
    fn at(this: Self, index: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match this
            .repr()
            .content()
            .get_hashed_by_value(index.get_hashed()?)
        {
            Some(v) => Ok(v.to_value()),
            None => Err(crate::Error::new_other(ValueError::KeyNotFound(
                index.to_repr(),
            ))),
        }
    }

    fn length(this: Self) -> i32 {
        this.repr().content().len() as i32
    }

    #[inline(always)]
    fn is_in(this: Self, other: Value<'v>) -> crate::Result<bool> {
        Ok(this
            .repr()
            .content()
            .contains_key_hashed_by_value(other.get_hashed()?))
    }

    unsafe fn iterate(this: Self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        unsafe {
            with_dict!(this.repr(), |d| d.iter_start())?;
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
            with_dict!(this.repr(), |d| d
                .content_unchecked()
                .keys()
                .nth(index)
                .copied())
        }
    }

    unsafe fn iter_stop(this: Self) {
        unsafe { with_dict!(this.repr(), |d| d.iter_stop()) }
    }

    fn set_at(this: Self, index: Value<'v>, alloc_value: Value<'v>) -> crate::Result<()> {
        let index = index.get_hashed()?;
        with_dict!(this.repr(), |d| d.set_at(index, alloc_value))
    }

    fn bit_or(this: Self, rhs: Self) -> Dict<'v> {
        let rhs = rhs.borrowed();
        let content = this.repr().content();
        if content.is_empty() {
            return (*rhs).clone();
        }
        // Might be faster if we preallocate the capacity, but then copying in the LHS
        // is more expensive and might oversize given the behaviour on duplicates.
        // If this becomes a bottleneck, benchmark.
        let mut items = content.clone();
        for (k, v) in rhs.iter_hashed() {
            items.insert_hashed(k, v);
        }
        Dict::new(items)
    }

    fn get_type_starlark_repr() -> Ty {
        Ty::any_dict()
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for DictGen<DictCell<'v>> {
    type Type = StarlarkDict<'v>;
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
    type Type = StarlarkDict<'v>;
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
    use std::cell::Cell;

    use crate::assert;
    use crate::collections::SmallMap;
    use crate::values::Heap;
    use crate::values::dict::Dict;
    use crate::values::dict::DictMut;
    use crate::values::dict::DictRef;
    use crate::values::dict::value::MutableDict;
    use crate::values::typing::type_compiled::matcher::TypeMatcher;
    use crate::values::typing::type_compiled::matchers::DictMatcherId;
    use crate::values::typing::type_compiled::matchers::IsDictOf;
    use crate::values::typing::type_compiled::matchers::IsInt;

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
            let k1 = heap.alloc("hello").get_hashed_value();
            let k2 = heap.alloc("world").get_hashed_value();
            let mut sm = SmallMap::new();
            sm.insert_hashed(k1, heap.alloc(12i32).to_value());
            sm.insert_hashed(k2, heap.alloc(56i32).to_value());
            let d = Dict::new(sm);

            assert_eq!(
                d.get(heap.alloc("hello").to_value())?.unwrap().unpack_i32(),
                Some(12)
            );
            assert_eq!(d.get(heap.alloc("foo").to_value())?, None);
            assert_eq!(d.get_str("hello").unwrap().unpack_i32(), Some(12));
            assert_eq!(d.get_str("foo"), None);
            Ok(())
        })
    }

    #[test]
    fn test_entries_a_matcher_accepted_are_not_tested_again() {
        Heap::temp(|heap| {
            let d = heap.alloc(Dict::new(SmallMap::new())).to_value();
            let cell = &d.downcast_ref::<MutableDict>().unwrap().0;
            // How many entries a check by `by` tests.
            let tested = |by: DictMatcherId| {
                let n = Cell::new(0);
                assert!(cell.all_match(
                    || Some(by),
                    |_, _| {
                        n.set(n.get() + 1);
                        true
                    }
                ));
                n.get()
            };
            let set = |k: i32, v: i32| {
                d.set_at(heap.alloc(k).to_value(), heap.alloc(v).to_value())
                    .unwrap()
            };
            let by = DictMatcherId::try_new().unwrap();
            let other = DictMatcherId::try_new().unwrap();

            // A short dict is tested whole every time.
            for i in 0..5 {
                set(i, i);
            }
            assert_eq!(5, tested(by));
            assert_eq!(5, tested(by));

            for i in 5..50 {
                set(i, i);
            }
            assert_eq!(50, tested(by));
            assert_eq!(0, tested(by));
            // New keys are the only entries to test.
            set(50, 50);
            set(51, 51);
            assert_eq!(2, tested(by));
            // A rejected entry is tested again, and so is everything after it.
            set(52, 52);
            assert!(!cell.all_match(|| Some(by), |_, _| false));
            assert_eq!(1, tested(by));
            // Another matcher starts over, and takes the place of the first.
            assert_eq!(53, tested(other));
            assert_eq!(53, tested(by));
            // Replacing a value, or any other way to change the dict, forgets it all.
            set(0, 100);
            assert_eq!(53, tested(by));
            let first = heap.alloc(0i32).to_value().get_hashed().unwrap();
            DictMut::from_value(d).unwrap().aref.remove_hashed(first);
            set(53, 53);
            assert_eq!(53, tested(by));
            assert_eq!(0, tested(by));
        })
    }

    #[test]
    fn test_a_dict_type_is_remembered_from_sixteen_entries() {
        Heap::temp(|heap| {
            let d = heap.alloc(Dict::new(SmallMap::new())).to_value();
            let cell = &d.downcast_ref::<MutableDict>().unwrap().0;
            let set = |k: i32, v: i32| {
                d.set_at(heap.alloc(k).to_value(), heap.alloc(v).to_value())
                    .unwrap()
            };
            let ints = IsDictOf::new(IsInt, IsInt);

            for i in 0..15 {
                set(i, i);
            }
            assert!(ints.matches(d));
            assert_eq!(None, cell.checked.get());

            set(15, 15);
            assert!(ints.matches(d));
            let (id, checked) = cell.checked.get().unwrap();
            assert_eq!(16, checked);

            // Reading the dict changes nothing, and neither does asking again.
            assert_eq!(16, DictRef::from_value(d).unwrap().iter().count());
            assert!(d.at(heap.alloc(3i32).to_value(), heap).is_ok());
            assert_eq!(Some((id, 16)), cell.checked.get());
            assert!(ints.matches(d));
            assert!(ints.clone().matches(d));
            assert_eq!(Some((id, 16)), cell.checked.get());

            set(16, 16);
            assert_eq!(Some((id, 16)), cell.checked.get());
            assert!(ints.matches(d));
            assert_eq!(Some((id, 17)), cell.checked.get());

            // Another matcher of the same type is another matcher.
            assert!(IsDictOf::new(IsInt, IsInt).matches(d));
            let (other, checked) = cell.checked.get().unwrap();
            assert_eq!(17, checked);
            assert_ne!(id, other);
        })
    }

    #[test]
    fn test_repr_cycle() {
        assert::eq("d = {}; d[17] = d; repr(d)", "'{17: {...}}'");
        assert::eq("d = {}; d[17] = d; str(d)", "'{17: {...}}'");
    }
}
