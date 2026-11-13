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

use starlark_derive::starlark_module;

use crate as starlark;
use crate::environment::MethodsBuilder;
use crate::values::Heap;
use crate::values::ValueOf;
use crate::values::types::bytes::value::StarlarkBytes;
use crate::values::typing::StarlarkIter;

/// An immutable sequence of bytes.
///
/// Bytes values are created with `b"..."` literals or the `bytes()` constructor.
/// They support indexing (`b[i]` returns an `int` in 0–255), slicing
/// (`b[i:j]` returns a `bytes`), membership (`x in b`), concatenation (`+`),
/// repetition (`*`), and `len()`.
///
/// ```
/// # starlark::assert::all_true(r#"
/// len(b"abc") == 3
/// b"abc"[0] == 97
/// b"abc"[1:] == b"bc"
/// 97 in b"abc"
/// b"ab" + b"cd" == b"abcd"
/// b"ab" * 2 == b"abab"
/// # "#);
/// ```
#[starlark_module]
pub(crate) fn bytes_methods(builder: &mut MethodsBuilder) {
    /// [bytes.elems](
    /// https://github.com/bazelbuild/starlark/blob/master/spec.md#bytes·elems
    /// ): returns an iterable of the bytes in the byte string, each as a
    /// length 1 value of type `bytes`.
    ///
    /// ```
    /// # starlark::assert::all_true(r#"
    /// list(b"abc".elems()) == [b"a", b"b", b"c"]
    /// # "#);
    /// ```
    fn elems<'v>(
        this: &'v StarlarkBytes,
        heap: Heap<'v>,
    ) -> anyhow::Result<ValueOf<'v, StarlarkIter<StarlarkBytes>>> {
        let it = this
            .as_bytes()
            .iter()
            .map(|&b| heap.alloc(StarlarkBytes::new(&[b])).to_value());
        Ok(ValueOf::new_unchecked(heap.alloc_tuple_iter(it)))
    }
}
