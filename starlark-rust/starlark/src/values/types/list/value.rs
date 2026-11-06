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
use std::cell::Cell;
use std::cmp;
use std::cmp::Ordering;
use std::convert::Infallible;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::mem;
use std::slice;

use allocative::Allocative;
use display_container::fmt_container;
use serde::Serialize;
use starlark_derive::StarlarkRepresentations;
use starlark_derive::Trace;
use starlark_derive::starlark_value;
use starlark_syntax::slice_vec_ext::SliceExt;
use starlark_syntax::slice_vec_ext::VecExt;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::environment::Methods;
use crate::hint::likely;
use crate::hint::unlikely;
use crate::private::Private;
use crate::static_starlark_value;
use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::array::Array;
use crate::values::comparison::compare_slice;
use crate::values::comparison::equals_slice;
use crate::values::error::ValueError;
use crate::values::index::apply_slice;
use crate::values::index::convert_index;
use crate::values::list::ListRef;
use crate::values::type_repr::StarlarkTypeRepr;

#[derive(Clone, Default, Trace, Debug, ProvidesStaticType, Allocative)]
#[repr(transparent)]
pub(crate) struct ListGen<T>(pub(crate) T);

/// Define the mutable list type.
#[derive(Trace, Debug, ProvidesStaticType, Allocative)]
pub(crate) struct ListData<'v> {
    /// The data stored by the list.
    #[allocative(skip)]
    pub(crate) content: Cell<ValueTyped<'v, Array<'v>>>,
}

/// Define the frozen list type. [`FrozenList`] is an alias for `ListGen<FrozenListData<'static>>`.
#[derive(ProvidesStaticType, Allocative)]
#[repr(C)]
pub(crate) struct FrozenListData<'v> {
    len: usize,
    /// The data stored by the list.
    content: [Value<'v>; 0],
}

impl<'v> Debug for FrozenListData<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrozenList")
            .field("content", &self.content())
            .finish()
    }
}

/// Runtime type of frozen list. Alias is used in `StarlarkDocs` derive.
pub(crate) type FrozenList = ListGen<FrozenListData<'static>>;

pub(crate) type List<'v> = ListGen<ListData<'v>>;

static_starlark_value!(pub(crate) VALUE_EMPTY_FROZEN_LIST: FrozenList = unsafe { ListGen(FrozenListData::new(0)) });

impl<'v> ListGen<FrozenListData<'v>> {
    pub(crate) fn offset_of_content() -> usize {
        mem::offset_of!(FrozenListData<'v>, content)
    }
}

impl<'v> ListData<'v> {
    #[inline]
    pub(crate) fn from_value_mut(x: Value<'v>) -> anyhow::Result<&'v Self> {
        #[derive(thiserror::Error, Debug)]
        #[error("Value is not list, value type: `{0}`")]
        struct NotListError(&'static str);

        #[cold]
        #[inline(never)]
        fn error<'v>(x: Value<'v>) -> anyhow::Error {
            if x.downcast_ref::<ListGen<FrozenListData<'v>>>().is_some() {
                ValueError::CannotMutateImmutableValue.into()
            } else {
                NotListError(x.get_type()).into()
            }
        }

        if let Some(x) = x.downcast_ref::<ListGen<ListData<'v>>>() {
            x.0.check_can_mutate()?;
            Ok(&x.0)
        } else {
            Err(error(x))
        }
    }

    pub(crate) unsafe fn from_value_unchecked_mut(x: Value<'v>) -> &'v Self {
        unsafe {
            let list = x.downcast_ref_unchecked::<ListGen<ListData<'v>>>();
            debug_assert!(list.0.check_can_mutate().is_ok());
            &list.0
        }
    }

    pub(crate) fn is_list_type(x: TypeId) -> bool {
        x == TypeId::of::<ListGen<ListData>>() || x == TypeId::of::<FrozenList>()
    }

    /// Return an error if there's at least one iterator over the list.
    fn check_can_mutate(&self) -> anyhow::Result<()> {
        if unlikely(self.content.get().as_ref().iter_count_is_non_zero()) {
            return Err(ValueError::MutationDuringIteration.into());
        }
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn reserve_additional_slow(&self, additional: usize, heap: Heap<'v>) {
        let new_cap = cmp::max(self.len() + additional, self.len() * 2);
        // Size of `Array` is 2 words and size of `List` is one word,
        // so allocating at least 4 words would not be too large waste.
        // Note `Vec` allocates 4 by default.
        // Also note `Array` removes extra capacity on GC.
        let new_cap = cmp::max(new_cap, 4);

        let new_array = heap.alloc_array(new_cap);
        new_array.extend_from_slice(self.content());
        self.content.set(new_array);
    }

    #[inline(always)]
    fn reserve_additional(&self, additional: usize, heap: Heap<'v>) {
        if likely(self.content.get().as_ref().remaining_capacity() >= additional) {
            return;
        }

        self.reserve_additional_slow(additional, heap);
    }

    pub(crate) fn double(&self, heap: Heap<'v>) {
        self.reserve_additional(self.len(), heap);
        self.content.get().double();
    }

    #[inline]
    pub(crate) fn extend<I: IntoIterator<Item = Value<'v>>>(&self, iter: I, heap: Heap<'v>) {
        match self.try_extend(iter.into_iter().map(Ok::<_, Infallible>), heap) {
            Ok(()) => {}
        }
    }

    #[inline]
    pub(crate) fn try_extend<E, I: IntoIterator<Item = Result<Value<'v>, E>>>(
        &self,
        iter: I,
        heap: Heap<'v>,
    ) -> Result<(), E> {
        let iter = iter.into_iter();
        let (lo, hi) = iter.size_hint();
        match hi {
            Some(hi) if lo == hi => {
                // Exact size iterator.
                self.reserve_additional(lo, heap);
                // Extend will panic if upper bound is provided incorrectly.
                self.content.get().try_extend(iter)?;
            }
            Some(hi) if self.content.get().remaining_capacity() >= hi => {
                // Enough capacity for upper bound.
                // Extend will panic if upper bound is provided incorrectly.
                self.content.get().try_extend(iter)?;
            }
            _ => {
                // Default slow version.
                self.reserve_additional(iter.size_hint().0, heap);
                for item in iter {
                    self.push(item?, heap);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn push(&self, value: Value<'v>, heap: Heap<'v>) {
        self.reserve_additional(1, heap);
        self.content.get().push(value);
    }

    pub(crate) fn clear(&self) {
        self.content.get().clear();
    }

    pub(crate) fn insert(&self, index: usize, value: Value<'v>, heap: Heap<'v>) {
        self.reserve_additional(1, heap);
        self.content.get().insert(index, value);
    }

    pub(crate) fn remove(&self, index: usize) -> Value<'v> {
        self.content.get().remove(index)
    }
}

impl<'v, V: AllocValue<'v>> AllocValue<'v> for Vec<V> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_list_iter(self.into_iter().map(|x| x.alloc_value(heap)))
    }
}

impl<'fv, V: AllocFrozenValue<'fv>> AllocFrozenValue<'fv> for Vec<V> {
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        heap.alloc_list(&self.into_map(|x| x.alloc_frozen_value(heap)))
    }
}

impl<'a, V: 'a> StarlarkTypeRepr for &'a [V]
where
    &'a V: StarlarkTypeRepr,
{
    type Canonical = <Vec<&'a V> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        Vec::<&'a V>::starlark_type_repr()
    }
}

impl<'a, 'v, V: 'a> AllocValue<'v> for &'a [V]
where
    &'a V: AllocValue<'v>,
{
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_list_iter(self.iter().map(|x| x.alloc_value(heap)))
    }
}

impl<'fv, 'a, V: 'a> AllocFrozenValue<'fv> for &'a [V]
where
    &'a V: AllocFrozenValue<'fv>,
{
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        heap.alloc_list(&self.map(|x| x.alloc_frozen_value(heap)))
    }
}

impl<'v> FrozenListData<'v> {
    pub(crate) const unsafe fn new(len: usize) -> FrozenListData<'v> {
        FrozenListData { len, content: [] }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn content(&self) -> &[Value<'v>] {
        unsafe { slice::from_raw_parts(self.content.as_ptr(), self.len) }
    }
}

impl<'v> ListData<'v> {
    /// The result of calling `type()` on lists.
    pub const TYPE: &'static str = "list";

    /// Type of list as frozen string value.
    pub fn get_type_value_static() -> StringValue<'static> {
        FrozenList::get_type_value_static()
    }

    pub(crate) fn new(content: ValueTyped<'v, Array<'v>>) -> Self {
        ListData {
            content: Cell::new(content),
        }
    }

    /// Obtain the length of the list.
    pub fn len(&self) -> usize {
        self.content.get().len()
    }

    /// List content.
    ///
    /// Note this operation does not prevent mutation of this list while
    /// holding the slice. But such mutation does not violate memory-safety.
    pub fn content(&self) -> &[Value<'v>] {
        self.content.get().as_ref().content()
    }
}

impl<'v> Display for ListData<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        display_list(self.content.get().content(), f)
    }
}

impl<'v> Display for FrozenListData<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        display_list(self.content(), f)
    }
}

// This trait need to be `pub(crate)` because `ListGen<T>` is.
pub(crate) trait ListLike<'v>: Debug + Allocative {
    fn content(&self) -> &[Value<'v>];
    fn set_at(&self, i: usize, v: Value<'v>) -> crate::Result<()>;

    // These functions are unsafe for the same reason
    // `StarlarkValue` iterator functions are unsafe.
    unsafe fn new_iter(&self, me: Value<'v>) -> Value<'v>;
    unsafe fn iter_size_hint(&self, index: usize) -> (usize, Option<usize>);
    unsafe fn iter_next(&self, index: usize) -> Option<Value<'v>>;
    unsafe fn iter_stop(&self);
}

impl<'v> ListLike<'v> for ListData<'v> {
    fn content(&self) -> &[Value<'v>] {
        self.content.get().as_ref().content()
    }

    fn set_at(&self, i: usize, v: Value<'v>) -> crate::Result<()> {
        self.check_can_mutate()?;
        self.content.get().set_at(i, v);
        Ok(())
    }

    unsafe fn new_iter(&self, _me: Value<'v>) -> Value<'v> {
        self.content.get().inc_iter_count();
        self.content.get().to_value()
    }

    unsafe fn iter_size_hint(&self, _index: usize) -> (usize, Option<usize>) {
        panic!("Iteration is performed on Array")
    }

    unsafe fn iter_next(&self, _index: usize) -> Option<Value<'v>> {
        panic!("Iteration is performed on Array")
    }

    unsafe fn iter_stop(&self) {
        panic!("Iteration is performed on Array")
    }
}

impl<'v> ListLike<'v> for FrozenListData<'v> {
    fn content(&self) -> &[Value<'v>] {
        self.content()
    }

    fn set_at(&self, _i: usize, _v: Value<'v>) -> crate::Result<()> {
        Err(crate::Error::new_other(
            ValueError::CannotMutateImmutableValue,
        ))
    }

    unsafe fn iter_size_hint(&self, index: usize) -> (usize, Option<usize>) {
        debug_assert!(index <= self.len());
        let rem = self.len() - index;
        (rem, Some(rem))
    }

    unsafe fn new_iter(&self, me: Value<'v>) -> Value<'v> {
        me
    }

    unsafe fn iter_next(&self, index: usize) -> Option<Value<'v>> {
        self.content().get(index).map(|v| v.to_value())
    }

    unsafe fn iter_stop(&self) {}
}

impl<T: Display> Display for ListGen<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

pub(crate) fn display_list(xs: &[Value], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt_container(f, "[", "]", xs.iter())
}

starlark::methods_static!(LIST_METHODS = crate::values::types::list::methods::list_methods);

/// A `list` in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub(crate) enum ListView<'v> {
    Frozen(ValueTyped<'v, ListGen<FrozenListData<'v>>>),
    Unfrozen(ValueTyped<'v, ListGen<ListData<'v>>>),
}

/// `$e` with `$x` bound to the representation's `ListLike`.
macro_rules! with_list {
    ($self:expr, |$x:ident| $e:expr) => {
        match $self {
            ListView::Frozen($x) => {
                let $x = &$x.as_ref().0;
                $e
            }
            ListView::Unfrozen($x) => {
                let $x = &$x.as_ref().0;
                $e
            }
        }
    };
}

impl<'v> ListView<'v> {
    #[inline]
    fn content(self) -> &'v [Value<'v>] {
        with_list!(self, |l| l.content())
    }
}

#[starlark_value(type = ListData::TYPE)]
impl<'v> StarlarkType<'v> for ListView<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(LIST_METHODS.methods())
    }

    fn collect_repr(&self, s: &mut String) {
        // Fast path as repr() for lists is quite hot
        s.push('[');
        for (i, v) in self.content().iter().enumerate() {
            if i != 0 {
                s.push_str(", ");
            }
            v.collect_repr(s);
        }
        s.push(']');
    }

    fn collect_repr_cycle(&self, collector: &mut String) {
        collector.push_str("[...]");
    }

    fn to_bool(&self) -> bool {
        !self.content().is_empty()
    }

    fn equals(&self, other: Value<'v>) -> crate::Result<bool> {
        match ListRef::from_value(other) {
            None => Ok(false),
            Some(other) => equals_slice(self.content(), &other.content, |x, y| x.equals(*y)),
        }
    }

    fn compare(&self, other: Value<'v>) -> crate::Result<Ordering> {
        match ListRef::from_value(other) {
            None => ValueError::unsupported_with(self, "cmp()", other),
            Some(other) => compare_slice(self.content(), &other.content, |x, y| x.compare(*y)),
        }
    }

    fn at(&self, index: Value, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        let content = self.content();
        let i = convert_index(index, content.len() as i32)? as usize;
        Ok(content[i])
    }

    fn length(&self) -> crate::Result<i32> {
        Ok(self.content().len() as i32)
    }

    fn is_in(&self, other: Value<'v>) -> crate::Result<bool> {
        for x in self.content().iter() {
            if x.equals(other)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn slice(
        &self,
        start: Option<Value>,
        stop: Option<Value>,
        stride: Option<Value>,
        heap: Heap<'v>,
    ) -> crate::Result<Value<'v>> {
        let xs = self.content();
        let res = apply_slice(xs, start, stop, stride)?;
        Ok(heap.alloc_list(&res))
    }

    unsafe fn iterate(&self, me: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        unsafe { Ok(with_list!(self, |l| l.new_iter(me))) }
    }

    unsafe fn iter_size_hint(&self, index: usize) -> (usize, Option<usize>) {
        unsafe { with_list!(self, |l| l.iter_size_hint(index)) }
    }

    unsafe fn iter_next(&self, index: usize, _heap: Heap<'v>) -> Option<Value<'v>> {
        unsafe { with_list!(self, |l| l.iter_next(index)) }
    }

    unsafe fn iter_stop(&self) {
        unsafe { with_list!(self, |l| l.iter_stop()) }
    }

    fn add(&self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        ListRef::from_value(other)
            .map(|other| Ok(heap.alloc_list_concat(self.content(), other.content())))
    }

    fn mul(&self, other: Value, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        let l = match i32::unpack_value(other) {
            Ok(Some(l)) => l,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        let content = self.content();
        let mut result = Vec::with_capacity(content.len() * cmp::max(0, l) as usize);
        for _ in 0..l {
            result.extend(content.iter());
        }
        Some(Ok(heap.alloc_list(&result)))
    }

    fn rmul(&self, lhs: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        self.mul(lhs, heap)
    }

    fn set_at(&self, index: Value<'v>, alloc_value: Value<'v>) -> crate::Result<()> {
        let i = convert_index(index, self.content().len() as i32)? as usize;
        with_list!(self, |l| l.set_at(i, alloc_value))
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(Ty::any_list())
    }

    fn get_type_starlark_repr() -> Ty {
        Ty::any_list()
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for ListGen<ListData<'v>> {
    type Canonical = ListGen<FrozenListData<'v>>;
    type Type = ListView<'v>;

    fn is_special(_: Private) -> bool
    where
        Self: Sized,
    {
        true
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for ListGen<FrozenListData<'v>> {
    type Type = ListView<'v>;

    fn is_special(_: Private) -> bool
    where
        Self: Sized,
    {
        true
    }
}

impl<'v, T: ListLike<'v>> Serialize for ListGen<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.0.content().iter())
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;
    use crate::assert::Assert;

    #[test]
    fn test_to_str() {
        assert::all_true(
            r#"
str([1, 2, 3]) == "[1, 2, 3]"
str([1, [2, 3]]) == "[1, [2, 3]]"
str([]) == "[]"
"#,
        );
    }

    #[test]
    fn test_repr_cycle() {
        assert::eq("l = []; l.append(l); repr(l)", "'[[...]]'");
        assert::eq("l = []; l.append(l); str(l)", "'[[...]]'");
    }

    #[test]
    fn test_mutate_list() {
        assert::is_true(
            r#"
v = [1, 2, 3]
v[1] = 1
v[2] = [2, 3]
v == [1, 1, [2, 3]]
"#,
        );
    }

    #[test]
    fn test_arithmetic_on_list() {
        assert::all_true(
            r#"
[1, 2, 3] + [2, 3] == [1, 2, 3, 2, 3]
[1, 2, 3] * 3 == [1, 2, 3, 1, 2, 3, 1, 2, 3]
"#,
        );
    }

    #[test]
    fn test_value_alias() {
        assert::is_true(
            r#"
v1 = [1, 2, 3]
v2 = v1
v2[2] = 4
v1 == [1, 2, 4] and v2 == [1, 2, 4]
"#,
        );
    }

    #[test]
    fn test_mutating_imports() {
        let mut a = Assert::new();
        a.module(
            "x",
            r#"
frozen_list = [1, 2]
frozen_list += [4]
def frozen_list_result():
    return frozen_list
def list_result():
    return [1, 2, 4]
"#,
        );
        a.fail("load('x','frozen_list')\nfrozen_list += [1]", "Immutable");
        a.fail(
            "load('x','frozen_list_result')\nx = frozen_list_result()\nx += [1]",
            "Immutable",
        );
        a.is_true("load('x','list_result')\nx = list_result()\nx += [8]\nx == [1, 2, 4, 8]");
    }

    #[test]
    fn test_compare() {
        // Lexicographic comparison.
        assert::is_true("[1, 2] < [10]");
    }
}
