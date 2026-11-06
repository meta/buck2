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
use std::iter;
use std::slice;

use ref_cast::RefCastCustom;
use ref_cast::ref_cast_custom;

use crate::typing::Ty;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::tuple::UnpackTuple;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::types::tuple::value::Tuple;

/// Reference to tuple data in Starlark heap.
#[derive(RefCastCustom, Debug)]
#[repr(transparent)]
pub struct TupleRef<'v> {
    contents: [Value<'v>],
}

impl<'v> TupleRef<'v> {
    /// `type(())`, which is `"tuple"`.
    pub const TYPE: &'static str = Tuple::<'v>::TYPE;

    #[ref_cast_custom]
    fn new(slice: &'v [Value<'v>]) -> &'v TupleRef<'v>;

    /// Downcast a value to a tuple.
    #[inline]
    pub fn from_value(value: Value<'v>) -> Option<&'v TupleRef<'v>> {
        Some(Self::new(Tuple::from_value(value)?.content()))
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        self.contents.len()
    }

    /// Tuple elements.
    pub fn content(&self) -> &[Value<'v>] {
        &self.contents
    }

    /// Iterate over the contents.
    pub fn iter<'a>(&'a self) -> iter::Copied<slice::Iter<'a, Value<'v>>> {
        self.content().iter().copied()
    }
}

impl<'v> StarlarkTypeRepr for &'v TupleRef<'v> {
    type Canonical = <UnpackTuple<Value<'static>> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        Ty::any_tuple()
    }
}

impl<'v> UnpackValue<'v> for &'v TupleRef<'v> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(TupleRef::from_value(value))
    }
}
