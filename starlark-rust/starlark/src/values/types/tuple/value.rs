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

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::mem;

use allocative::Allocative;
use display_container::fmt_container;
use serde::Serialize;
use serde::ser::SerializeTuple;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::collections::StarlarkHasher;
use crate::private::Private;
use crate::static_starlark_value;
use crate::typing::Ty;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::comparison::compare_slice;
use crate::values::comparison::equals_slice;
use crate::values::function::SpecialBuiltinFunction;
use crate::values::index::apply_slice;
use crate::values::index::convert_index;
use crate::values::layout::avalue::TrailingValues;
use crate::values::tuple::AllocTuple;
use crate::values::tuple::StarlarkTuple;
use crate::values::typing::StarlarkIter;

/// Define the tuple type.
#[repr(C)]
#[derive(ProvidesStaticType, Allocative)]
pub(crate) struct Tuple<'v> {
    len: usize,
    /// The data stored by the tuple.
    content: [Value<'v>; 0],
}

impl<'v> Display for Tuple<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // For single-item tuples we need to add a trailing ',' and easier to just handle that ourself than configure display_container correctly
        if self.len() == 1 {
            if f.alternate() {
                write!(f, "( {:#}, )", self.content()[0])
            } else {
                write!(f, "({},)", self.content()[0])
            }
        } else {
            fmt_container(f, "(", ")", self.content().iter())
        }
    }
}

impl<'v> Debug for Tuple<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("TupleGen")
            .field("content", &self.content())
            .finish()
    }
}

impl<'v> Tuple<'v> {
    /// `type(())`.
    pub(crate) const TYPE: &'static str = "tuple";

    pub(crate) const unsafe fn new(len: usize) -> Tuple<'v> {
        Tuple { len, content: [] }
    }

    pub(crate) fn offset_of_content() -> usize {
        mem::offset_of!(Self, content)
    }
}

static_starlark_value!(pub(crate) VALUE_EMPTY_TUPLE: Tuple<'static> = unsafe { Tuple::new(0) });

/// Runtime type of frozen tuple.
impl<'v> Tuple<'v> {
    /// Downcast a value to a tuple.
    pub(crate) fn from_value(value: Value<'v>) -> Option<&'v Self> {
        value.downcast_ref::<Self>()
    }
}

// SAFETY: every tuple allocation writes `len` elements after `content`.
unsafe impl<'v> TrailingValues<'v> for Tuple<'v> {
    fn len(&self) -> usize {
        self.len
    }

    fn content_ptr(&self) -> *const Value<'v> {
        self.content.as_ptr()
    }

    fn content_mut_ptr(&mut self) -> *mut Value<'v> {
        self.content.as_mut_ptr()
    }
}

impl<'v> Tuple<'v> {
    /// Get the length of the tuple.
    pub(crate) fn len(&self) -> usize {
        TrailingValues::len(self)
    }

    /// Tuple elements.
    pub(crate) fn content(&self) -> &[Value<'v>] {
        TrailingValues::content(self)
    }

    /// Iterate over the elements of the tuple.
    pub(crate) fn iter<'a>(&'a self) -> impl Iterator<Item = Value<'v>> + 'a
    where
        'v: 'a,
    {
        self.content().iter().map(|e| e.to_value())
    }
}

/// A tuple of the items of `tuple` `times` times over: `tuple * times` and `times * tuple`.
// Inlined into `mul` and `rmul`: as a call it measured 13 instructions more on `t * 3`.
#[inline(always)]
fn repeated<'v>(
    tuple: StarlarkTuple<'v>,
    times: i32,
    heap: Heap<'v>,
) -> ValueOf<'v, StarlarkTuple<'v>> {
    let mut result = Vec::new();
    for _i in 0..times {
        result.extend(tuple.values());
    }
    heap.alloc_tuple(&result)
}

#[starlark_value(type = Tuple::TYPE, type_expr = by_hand)]
impl<'v> StarlarkType<'v> for StarlarkTuple<'v> {
    /// [tuple](
    /// https://github.com/bazelbuild/starlark/blob/master/spec.md#tuple
    /// ): returns a tuple containing the elements of the iterable x.
    ///
    /// With no arguments, `tuple()` returns the empty tuple.
    ///
    /// ```
    /// # starlark::assert::all_true(r#"
    /// tuple() == ()
    /// tuple([1,2,3]) == (1, 2, 3)
    /// # "#);
    /// ```
    #[starlark(
        constructor,
        speculative_exec_safe,
        special_builtin_function = SpecialBuiltinFunction::Tuple,
    )]
    fn tuple(
        #[starlark(require = pos)] a: Option<ValueOf<'v, StarlarkIter<Value<'v>>>>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkTuple<'v>>> {
        if let Some(a) = a {
            if let Some(tuple) = ValueOf::new(a.to_value()) {
                return Ok(tuple);
            }

            let it = a.to_value().iterate(heap)?;
            Ok(heap.alloc_tuple_iter(it.map(ValueOf::from)))
        } else {
            Ok(heap.alloc(AllocTuple::EMPTY))
        }
    }

    fn to_bool(this: Self) -> bool {
        !this.is_empty()
    }

    fn write_hash(this: Self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        for v in this.content() {
            v.write_hash(hasher)?;
        }
        Ok(())
    }

    fn equals(this: Self, other: Self) -> crate::Result<bool> {
        equals_slice(this.content(), other.content(), |x, y| x.equals(*y))
    }

    fn compare(this: Self, other: Self) -> crate::Result<Ordering> {
        compare_slice(this.content(), other.content(), |x, y| x.compare(*y))
    }

    fn at(this: Self, index: i32) -> crate::Result<Value<'v>> {
        let content = this.content();
        let i = convert_index(index, content.len() as i32)? as usize;
        Ok(content[i])
    }

    fn length(this: Self) -> i32 {
        this.len() as i32
    }

    fn is_in(this: Self, other: Value<'v>) -> crate::Result<bool> {
        for x in this.content() {
            if x.equals(other)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn slice(
        this: Self,
        start: Option<Value>,
        stop: Option<Value>,
        stride: Option<Value>,
        heap: Heap<'v>,
    ) -> crate::Result<Value<'v>> {
        Ok(heap
            .alloc_tuple(&apply_slice(this.content(), start, stop, stride)?)
            .to_value())
    }

    unsafe fn iterate(this: Self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(this.to_value())
    }

    unsafe fn iter_size_hint(this: Self, index: usize) -> (usize, Option<usize>) {
        debug_assert!(index <= this.len());
        let rem = this.len() - index;
        (rem, Some(rem))
    }

    unsafe fn iter_next(this: Self, index: usize, _heap: Heap<'v>) -> Option<Value<'v>> {
        this.content().get(index).copied()
    }

    unsafe fn iter_stop(_this: Self) {}

    fn add(this: Self, other: Self, heap: Heap<'v>) -> ValueOf<'v, StarlarkTuple<'v>> {
        // The slices are taken once: read through the tuples again after the allocation, the
        // lengths are loaded a second time.
        let (this, other) = (this.content(), other.content());
        let mut result = Vec::with_capacity(this.len() + other.len());
        for x in this {
            result.push(*x);
        }
        for x in other {
            result.push(*x);
        }
        heap.alloc_tuple(&result)
    }

    fn mul(this: Self, times: i32, heap: Heap<'v>) -> ValueOf<'v, StarlarkTuple<'v>> {
        repeated(this, times, heap)
    }

    fn rmul(this: Self, times: i32, heap: Heap<'v>) -> ValueOf<'v, StarlarkTuple<'v>> {
        repeated(this, times, heap)
    }

    fn collect_repr_cycle(_this: Self, collector: &mut String) {
        collector.push_str("(...)");
    }

    fn get_type_starlark_repr() -> Ty {
        Ty::any_tuple()
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for Tuple<'v> {
    type Type = StarlarkTuple<'v>;

    fn is_special(_: Private) -> bool
    where
        Self: Sized,
    {
        true
    }
}

impl<'v> Serialize for Tuple<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut tuple_serializer = serializer.serialize_tuple(self.len)?;

        for e in self.content().iter() {
            tuple_serializer.serialize_element(e)?;
        }

        tuple_serializer.end()
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
str((1, 2, 3)) == "(1, 2, 3)"
str((1, (2, 3))) == "(1, (2, 3))"
str((1,)) == "(1,)"
"#,
        );
    }

    #[test]
    fn test_repr_cycle() {
        let mut a = Assert::new();
        // TODO(nga): fix and enable.
        a.disable_static_typechecking();
        a.eq("l = []; t = (l,); l.append(t); repr(t)", "'([(...)],)'");
        a.eq("l = []; t = (l,); l.append(t); str(t)", "'([(...)],)'");
    }

    #[test]
    fn test_tuple_ellipsis_runtime() {
        assert::is_true("isinstance((), tuple[int, ...])");
        assert::is_true("isinstance((1, ), tuple[int, ...])");
        assert::is_true("isinstance((1, 2), tuple[int, ...])");
        assert::is_false("isinstance(('x', 2), tuple[int, ...])");
    }
}
