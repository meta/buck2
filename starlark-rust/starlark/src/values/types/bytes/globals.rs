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
use crate::environment::GlobalsBuilder;
use crate::values::Heap;
use crate::values::Value;
use crate::values::types::bytes::value::StarlarkBytes;

#[starlark_module]
pub(crate) fn register_bytes(globals: &mut GlobalsBuilder) {
    /// [bytes](
    /// https://github.com/bazelbuild/starlark/blob/master/spec.md#bytes
    /// ): construct a bytes value.
    ///
    /// `bytes(x)` converts its argument to a `bytes` value.
    ///
    /// If `x` is already a `bytes` value, the result is `x`.
    ///
    /// If `x` is a string, its UTF-8 encoding is returned.
    ///
    /// If `x` is an iterable of integers (each in 0–255), the bytes are
    /// constructed from those integer values.
    ///
    /// ```
    /// # starlark::assert::all_true(r#"
    /// bytes(b"hello") == b"hello"
    /// bytes("hello") == b"hello"
    /// bytes([104, 101, 108, 108, 111]) == b"hello"
    /// # "#);
    /// ```
    #[starlark(as_type = StarlarkBytes)]
    fn bytes<'v>(
        #[starlark(require = pos)] x: Value<'v>,
        heap: Heap<'v>,
    ) -> anyhow::Result<Value<'v>> {
        if StarlarkBytes::from_value(x).is_some() {
            return Ok(x);
        }
        if let Some(s) = x.unpack_str() {
            return Ok(heap
                .alloc_representation(StarlarkBytes::new(s.as_bytes()))
                .to_value());
        }
        // Try iterating as a sequence of ints
        let mut result = Vec::new();
        let iter = x.iterate(heap).map_err(|_| {
            anyhow::anyhow!(
                "bytes() argument must be bytes, str, or iterable of int, not '{}'",
                x.get_type()
            )
        })?;
        for v in iter {
            match v.unpack_i32() {
                Some(n) if n >= 0 && n <= 255 => result.push(n as u8),
                Some(n) => {
                    return Err(anyhow::anyhow!(
                        "bytes() integer {} is out of range 0..=255",
                        n
                    ));
                }
                None => {
                    return Err(anyhow::anyhow!(
                        "bytes() iterable must contain integers, not '{}'",
                        v.get_type()
                    ));
                }
            }
        }
        Ok(heap
            .alloc_representation(StarlarkBytes::from_vec(result))
            .to_value())
    }
}
