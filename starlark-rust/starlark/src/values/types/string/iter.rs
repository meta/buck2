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

//! Implementation of iterators for string type.

use allocative::Allocative;
use derive_more::Display;
use starlark_derive::Freeze;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::Trace;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::starlark_complex_value;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::string::StarlarkStr;
use crate::values::types::int::view::IntView;
use crate::values::typing::iter::StarlarkIter;

/// An opaque iterator over a string, produced by elems/codepoints
#[derive(
    Debug,
    Trace,
    Display,
    Freeze,
    NoSerialize,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[display("iterator")]
#[repr(C)]
struct StringIterable<'v> {
    string: StringValue<'v>,
    produce_char: bool, // if not char, then int
}

starlark_complex_value!(StringIterable);

pub(crate) fn iterate_chars<'v>(
    string: StringValue<'v>,
    heap: Heap<'v>,
) -> ValueOf<'v, StarlarkIter<StarlarkStr>> {
    ValueOf::new(heap.alloc_complex(StringIterable {
        string,
        produce_char: true,
    }))
    .expect("a `StringIterable` iterates")
}

pub(crate) fn iterate_codepoints<'v>(
    string: StringValue<'v>,
    heap: Heap<'v>,
) -> ValueOf<'v, StarlarkIter<IntView<'v>>> {
    ValueOf::new(heap.alloc_complex(StringIterable {
        string,
        produce_char: false,
    }))
    .expect("a `StringIterable` iterates")
}

#[starlark_value(type = "iterator")]
impl<'v> StarlarkValue<'v> for StringIterable<'v> {
    unsafe fn iterate(this: &Self, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        // Lazy implementation: we allocate a tuple and then iterate over it.
        let iter = if this.produce_char {
            heap.alloc_tuple_iter(this.string.as_str().chars().map(|c| heap.alloc(c)))
                .to_value()
        } else {
            heap.alloc_tuple_iter(
                this.string
                    .as_str()
                    .chars()
                    .map(|c| heap.alloc(u32::from(c))),
            )
            .to_value()
        };
        Ok(iter)
    }
}
