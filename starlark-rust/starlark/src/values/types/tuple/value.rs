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
use crate::values::StarlarkValue;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::comparison::compare_slice;
use crate::values::comparison::equals_slice;
use crate::values::index::apply_slice;
use crate::values::index::convert_index;
use crate::values::layout::avalue::TrailingValues;

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

#[starlark_value(type = Tuple::TYPE)]
impl<'v> StarlarkValue<'v> for Tuple<'v> {
    fn is_special(_: Private) -> bool
    where
        Self: Sized,
    {
        true
    }

    fn to_bool(&self) -> bool {
        self.len() != 0
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        for v in self.content() {
            v.write_hash(hasher)?;
        }
        Ok(())
    }

    fn equals(&self, other: Value<'v>) -> crate::Result<bool> {
        match Tuple::from_value(other) {
            None => Ok(false),
            Some(other) => equals_slice(self.content(), other.content(), |x, y| x.equals(*y)),
        }
    }

    fn compare(&self, other: Value<'v>) -> crate::Result<Ordering> {
        match Tuple::from_value(other) {
            None => ValueError::unsupported_with(self, "cmp()", other),
            Some(other) => compare_slice(self.content(), other.content(), |x, y| x.compare(*y)),
        }
    }

    fn at(&self, index: Value, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        let i = convert_index(index, self.len() as i32)? as usize;
        Ok(self.content()[i].to_value())
    }

    fn length(&self) -> crate::Result<i32> {
        Ok(self.len() as i32)
    }

    fn is_in(&self, other: Value<'v>) -> crate::Result<bool> {
        for x in self.content() {
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
        Ok(heap.alloc_tuple(&apply_slice(self.content(), start, stop, stride)?))
    }

    unsafe fn iterate(&self, me: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(me)
    }

    unsafe fn iter_size_hint(&self, index: usize) -> (usize, Option<usize>) {
        debug_assert!(index <= self.len());
        let rem = self.len() - index;
        (rem, Some(rem))
    }

    unsafe fn iter_next(&self, index: usize, _heap: Heap<'v>) -> Option<Value<'v>> {
        self.content().get(index).map(|v| v.to_value())
    }

    unsafe fn iter_stop(&self) {}

    fn add(&self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        if let Some(other) = Tuple::from_value(other) {
            let mut result = Vec::with_capacity(self.len() + other.len());
            for x in self.iter() {
                result.push(x);
            }
            for x in other.iter() {
                result.push(x);
            }
            Some(Ok(heap.alloc_tuple(&result)))
        } else {
            None
        }
    }

    fn mul(&self, other: Value, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        let l = match i32::unpack_value(other) {
            Ok(Some(l)) => l,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        let mut result = Vec::new();
        for _i in 0..l {
            result.extend(self.content().iter().map(|e| e.to_value()));
        }
        Some(Ok(heap.alloc_tuple(&result)))
    }

    fn rmul(&self, lhs: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        self.mul(lhs, heap)
    }

    fn collect_repr_cycle(&self, collector: &mut String) {
        collector.push_str("(...)");
    }

    fn get_type_starlark_repr() -> Ty {
        Ty::any_tuple()
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
