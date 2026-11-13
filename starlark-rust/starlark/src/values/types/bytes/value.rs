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

use std::cmp;
use std::cmp::Ordering;
use std::fmt;
use std::fmt::Display;
use std::fmt::Write;
use std::hash::Hash;

use allocative::Allocative;
use serde::Serialize;
use serde::Serializer;
use starlark_derive::ProvidesStaticType;
use starlark_derive::starlark_value;
use starlark_map::StarlarkHasher;

use crate as starlark;
use crate::environment::Methods;
use crate::starlark_simple_value;
use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::index::apply_slice;
use crate::values::index::convert_index;

/// The result of calling `type()` on bytes.
pub const BYTES_TYPE: &str = "bytes";

/// An immutable sequence of bytes (integers in range 0–255).
///
/// Values of this type are created via `b"..."` literals or the `bytes()` constructor.
#[derive(
    Clone,
    Debug,
    ProvidesStaticType,
    Allocative,
    pagable::Pagable,
    crate::StarlarkPagableViaPagable
)]
pub struct StarlarkBytes {
    content: Box<[u8]>,
}

starlark_simple_value!(StarlarkBytes);

impl<'v> AllocValue<'v> for &'_ [u8] {
    type Type = StarlarkBytes;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::alloc(heap, StarlarkBytes::new(self))
    }
}

impl<'fv> AllocFrozenValue<'fv> for &'_ [u8] {
    type Type = StarlarkBytes;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        ValueOf::alloc_frozen(heap, StarlarkBytes::new(self))
    }
}

impl<'v> AllocValue<'v> for Vec<u8> {
    type Type = StarlarkBytes;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::alloc(heap, StarlarkBytes::from_vec(self))
    }
}

impl<'fv> AllocFrozenValue<'fv> for Vec<u8> {
    type Type = StarlarkBytes;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        ValueOf::alloc_frozen(heap, StarlarkBytes::from_vec(self))
    }
}

impl StarlarkBytes {
    /// Create a new bytes value from a byte slice.
    pub fn new(content: &[u8]) -> Self {
        StarlarkBytes {
            content: content.into(),
        }
    }

    /// Create a new bytes value from a `Vec<u8>`, without copying.
    pub fn from_vec(content: Vec<u8>) -> Self {
        StarlarkBytes {
            content: content.into_boxed_slice(),
        }
    }

    /// The raw bytes content.
    pub fn as_bytes(&self) -> &[u8] {
        &self.content
    }

    /// The length in bytes.
    pub fn len(&self) -> usize {
        self.content.len()
    }

    /// Whether the bytes are empty.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

/// Write a non-ASCII Unicode character in bytes repr format.
/// Alphanumeric chars (like "é", "世") are written as-is.
/// Non-alphanumeric chars (control, format, invisible, punctuation) are escaped as \uXXXX / \UXXXXXXXX.
/// This mirrors the `need_escape` logic in `string/repr.rs`.
fn write_non_ascii_char(f: &mut fmt::Formatter<'_>, c: char) -> fmt::Result {
    if c.is_alphanumeric() {
        f.write_char(c)
    } else if (c as u32) <= 0xFFFF {
        write!(f, "\\u{:04x}", c as u32)
    } else {
        write!(f, "\\U{:08x}", c as u32)
    }
}

impl Display for StarlarkBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("b\"")?;
        let content = &self.content;
        let mut i = 0;
        while i < content.len() {
            let byte = content[i];
            if byte < 0x80 {
                match byte {
                    b'"' => f.write_str("\\\"")?,
                    b'\\' => f.write_str("\\\\")?,
                    b'\n' => f.write_str("\\n")?,
                    b'\r' => f.write_str("\\r")?,
                    b'\t' => f.write_str("\\t")?,
                    0x20..=0x7e => f.write_char(byte as char)?,
                    _ => write!(f, "\\x{:02x}", byte)?,
                }
                i += 1;
            } else {
                // A UTF-8 character is at most 4 bytes, so only validate that far;
                // validating the whole remainder for every character is quadratic.
                let end = cmp::min(content.len(), i + 4);
                match std::str::from_utf8(&content[i..end]) {
                    Ok(s) => {
                        let c = s.chars().next().unwrap();
                        write_non_ascii_char(f, c)?;
                        i += c.len_utf8();
                    }
                    Err(e) => {
                        if e.valid_up_to() > 0 {
                            // There's a valid UTF-8 prefix; process just the first char.
                            let valid =
                                std::str::from_utf8(&content[i..i + e.valid_up_to()]).unwrap();
                            let c = valid.chars().next().unwrap();
                            write_non_ascii_char(f, c)?;
                            i += c.len_utf8();
                        } else {
                            // Invalid byte at position i.
                            write!(f, "\\x{:02x}", byte)?;
                            i += 1;
                        }
                    }
                }
            }
        }
        f.write_str("\"")
    }
}

impl Serialize for StarlarkBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(&self.content)
    }
}

starlark::methods_static!(BYTES_METHODS = crate::values::types::bytes::methods::bytes_methods);

#[starlark_value(type = BYTES_TYPE)]
impl<'v> StarlarkValue<'v> for StarlarkBytes {
    fn get_methods() -> Option<&'static Methods> {
        Some(BYTES_METHODS.methods())
    }

    /// repr(b"...") — byte literal syntax.
    fn collect_repr(&self, collector: &mut String) {
        write!(collector, "{self}").unwrap()
    }

    /// str(b"...") — UTF-8 decode, replacing invalid sequences with U+FFFD.
    fn collect_str(&self, collector: &mut String) {
        collector.push_str(&String::from_utf8_lossy(&self.content))
    }

    fn to_bool(&self) -> bool {
        !self.content.is_empty()
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        self.content.hash(hasher);
        Ok(())
    }

    fn equals(&self, other: Value<'v>) -> crate::Result<bool> {
        if let Some(other) = StarlarkBytes::from_value(other) {
            Ok(self.content == other.content)
        } else {
            Ok(false)
        }
    }

    fn compare(&self, other: Value<'v>) -> crate::Result<Ordering> {
        if let Some(other) = StarlarkBytes::from_value(other) {
            Ok(self.content.cmp(&other.content))
        } else {
            crate::values::ValueError::unsupported_with(self, "cmp()", other)
        }
    }

    fn length(&self) -> crate::Result<i32> {
        Ok(self.content.len() as i32)
    }

    /// `bytes[i]` returns the integer value of byte at index `i`.
    fn at(&self, index: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        let i = convert_index(index, self.content.len() as i32)? as usize;
        Ok(heap.alloc(self.content[i] as i32).to_value())
    }

    /// `bytes[i:j:k]` returns a new bytes object.
    fn slice(
        &self,
        start: Option<Value<'v>>,
        stop: Option<Value<'v>>,
        stride: Option<Value<'v>>,
        heap: Heap<'v>,
    ) -> crate::Result<Value<'v>> {
        let sliced = apply_slice(&self.content, start, stop, stride)?;
        Ok(heap
            .alloc_representation(StarlarkBytes::from_vec(sliced))
            .to_value())
    }

    /// `x in bytes`: supports both int (byte value) and bytes (subsequence).
    fn is_in(&self, other: Value<'v>) -> crate::Result<bool> {
        if let Some(n) = other.unpack_i32() {
            if n < 0 || n > 255 {
                return Err(crate::Error::new_other(anyhow::anyhow!(
                    "byte value must be in range 0..=255, got {}",
                    n
                )));
            }
            Ok(self.content.contains(&(n as u8)))
        } else if let Some(other_bytes) = StarlarkBytes::from_value(other) {
            let needle = &other_bytes.content;
            if needle.is_empty() {
                return Ok(true);
            }
            Ok(self
                .content
                .windows(needle.len())
                .any(|window| window == needle.as_ref()))
        } else {
            Err(crate::Error::new_other(anyhow::anyhow!(
                "argument to 'in bytes' must be an int or bytes, not '{}'",
                other.get_type()
            )))
        }
    }

    /// `bytes + bytes` — concatenation.
    fn add(&self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        if let Some(other) = StarlarkBytes::from_value(other) {
            let mut result = Vec::with_capacity(self.content.len() + other.content.len());
            result.extend_from_slice(&self.content);
            result.extend_from_slice(&other.content);
            Some(Ok(heap
                .alloc_representation(StarlarkBytes::from_vec(result))
                .to_value()))
        } else {
            None
        }
    }

    /// `bytes * n` — repetition.
    fn mul(&self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        let l = match i32::unpack_value(other) {
            Ok(Some(l)) => l,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        let count = std::cmp::max(0, l) as usize;
        let result = self.content.repeat(count);
        Some(Ok(heap
            .alloc_representation(StarlarkBytes::from_vec(result))
            .to_value()))
    }

    fn rmul(&self, lhs: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        self.mul(lhs, heap)
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(Ty::starlark_value::<Self>())
    }
}
