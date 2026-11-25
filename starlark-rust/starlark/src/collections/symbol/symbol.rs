/*
 * Copyright 2019 The Starlark in Rust Authors.
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

use std::fmt;
use std::fmt::Debug;
use std::mem;
use std::ptr::copy_nonoverlapping;
use std::slice;
use std::str;

use allocative::Allocative;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagableViaPagable;
use starlark_map::Hashed;
use starlark_map::StarlarkHashValue;

use crate as starlark;
use crate::collections::aligned_padded_str::AlignedPaddedStr;
use crate::values::Trace;
use crate::values::Tracer;

/// A pre-hashed string used for efficient dictionary lookup.
///
/// A string of up to sixteen bytes, which nearly every name is, is held in the symbol itself:
/// there is no allocation for it, and comparing it reads nothing but the symbol.
#[derive(ProvidesStaticType, StarlarkPagableViaPagable)]
pub(crate) struct Symbol {
    hash: u64,
    len: u32,
    small_hash: StarlarkHashValue,
    payload: Payload,
}

/// The longest string that a [`Symbol`] holds in place, in bytes.
const INLINE_BYTES: usize = 16;
const INLINE_WORDS: usize = INLINE_BYTES / mem::size_of::<usize>();

/// The string of a [`Symbol`], zero padded to whole words: `inline` if it is at most
/// [`INLINE_BYTES`] long, and otherwise `heap`, which points to
/// `len.div_ceil(size_of::<usize>())` words that the symbol owns.
///
/// Every byte is initialized whichever it is, so that `inline` can always be read: see
/// [`Symbol::in_place`].
union Payload {
    inline: [usize; INLINE_WORDS],
    heap: *mut usize,
}

// SAFETY: The symbol owns what `heap` points to, as a `Box` would.
unsafe impl Send for Symbol {}
// SAFETY: As above.
unsafe impl Sync for Symbol {}

/// The first sixteen bytes of `name` as two little-endian words, zero padded. Together with
/// its length this is the whole of a name of up to sixteen bytes.
#[inline]
pub(crate) const fn name_head(name: &str) -> [u64; 2] {
    let bytes = name.as_bytes();
    let mut head = [0u64; 2];
    let mut i = 0;
    while i < bytes.len() && i < 16 {
        head[i / 8] |= (bytes[i] as u64) << (8 * (i % 8));
        i += 1;
    }
    head
}

impl Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl PartialEq for Symbol {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        if self.len != other.len {
            return false;
        }
        if self.is_inline() {
            // SAFETY: Both are as long, so both are inline. The padding is zero, so equal
            // strings have equal words.
            unsafe { self.payload.inline == other.payload.inline }
        } else {
            self.as_aligned_padded_str() == other.as_aligned_padded_str()
        }
    }
}

impl Eq for Symbol {}

impl Clone for Symbol {
    fn clone(&self) -> Symbol {
        let payload = if self.is_inline() {
            Payload {
                // SAFETY: `is_inline` says that this is the field in use.
                inline: unsafe { self.payload.inline },
            }
        } else {
            // SAFETY: A string that is not inline is `len_words` words behind `heap`.
            let words = unsafe { slice::from_raw_parts(self.payload.heap, self.len_words()) };
            Payload::heap(Box::<[usize]>::from(words))
        };
        Symbol {
            hash: self.hash,
            len: self.len,
            small_hash: self.small_hash,
            payload,
        }
    }
}

impl Drop for Symbol {
    fn drop(&mut self) {
        if !self.is_inline() {
            // SAFETY: A string that is not inline is a boxed slice of `len_words` words, which
            // `new_hashed` or `clone` leaked into `heap`.
            drop(unsafe {
                Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                    self.payload.heap,
                    self.len_words(),
                ))
            });
        }
    }
}

// SAFETY: A symbol holds no value.
unsafe impl<'v> Trace<'v> for Symbol {
    fn trace(&mut self, _tracer: &Tracer<'v>) {}
}

impl Allocative for Symbol {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        if !self.is_inline() {
            let mut visitor = visitor.enter_unique(
                allocative::Key::new("payload"),
                mem::size_of::<*mut usize>(),
            );
            visitor.visit_simple(
                allocative::Key::new("words"),
                self.len_words() * mem::size_of::<usize>(),
            );
            visitor.exit();
        }
        visitor.exit();
    }
}

impl Symbol {
    pub(crate) fn new(x: &str) -> Self {
        Self::new_hashed(Hashed::new(x))
    }

    pub(crate) fn new_hashed(x: Hashed<&str>) -> Self {
        let small_hash = x.hash();
        let hash = small_hash.promote();
        let len = x.key().len();
        let payload = if len <= INLINE_BYTES {
            let mut inline = [0usize; INLINE_WORDS]; // 0 pad it at the end
            // SAFETY: `inline` has room for `INLINE_BYTES` bytes.
            unsafe {
                copy_nonoverlapping(x.key().as_ptr(), inline.as_mut_ptr() as *mut u8, len);
            }
            Payload { inline }
        } else {
            let len_words = len.div_ceil(mem::size_of::<usize>());
            let mut words: Vec<usize> = vec![0; len_words]; // 0 pad it at the end
            // SAFETY: `words` has room for `len` bytes.
            unsafe {
                copy_nonoverlapping(x.key().as_ptr(), words.as_mut_ptr() as *mut u8, len);
            }
            Payload::heap(words.into_boxed_slice())
        };
        Self {
            hash,
            len: len.try_into().unwrap(),
            small_hash,
            payload,
        }
    }

    #[inline]
    pub(crate) fn hash(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn is_inline(&self) -> bool {
        self.len as usize <= INLINE_BYTES
    }

    #[inline]
    fn len_words(&self) -> usize {
        (self.len as usize).div_ceil(mem::size_of::<usize>())
    }

    /// The string's words: `len_words` of them, the last zero padded.
    #[inline]
    fn words(&self) -> *const usize {
        if self.is_inline() {
            // SAFETY: `is_inline` says that this is the field in use.
            unsafe { self.payload.inline.as_ptr() }
        } else {
            // SAFETY: As above, for the other field.
            unsafe { self.payload.heap }
        }
    }

    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        // All safe because we promise we started out with a str
        unsafe {
            let s = slice::from_raw_parts(self.words() as *const u8, self.len as usize);
            str::from_utf8_unchecked(s)
        }
    }

    #[inline]
    pub(crate) fn as_aligned_padded_str(&self) -> AlignedPaddedStr<'_> {
        unsafe { AlignedPaddedStr::new(self.len as usize, self.words()) }
    }

    #[inline]
    pub(crate) fn as_str_hashed(&self) -> Hashed<&str> {
        Hashed::new_unchecked(self.small_hash, self.as_str())
    }

    pub(crate) fn small_hash(&self) -> StarlarkHashValue {
        self.small_hash
    }

    /// The length of the string, in bytes.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len as usize
    }

    /// The sixteen bytes that the symbol holds in place, as two little-endian words. For a
    /// string of at most sixteen bytes they are the string as [`name_head`] gives it, and with
    /// the length that is the whole string: a methods table compares such a name by this,
    /// which reads nothing but the symbol. For a longer string they mean nothing.
    #[inline]
    pub(crate) fn in_place(&self) -> [u64; 2] {
        // SAFETY: Every byte of the payload is initialized, and it is sixteen bytes.
        let bytes: [u8; INLINE_BYTES] =
            unsafe { mem::transmute::<[usize; INLINE_WORDS], _>(self.payload.inline) };
        let (low, high) = bytes.split_at(8);
        [
            u64::from_le_bytes(low.try_into().unwrap()),
            u64::from_le_bytes(high.try_into().unwrap()),
        ]
    }
}

impl Payload {
    fn heap(words: Box<[usize]>) -> Payload {
        let mut payload = Payload {
            inline: [0; INLINE_WORDS],
        };
        payload.heap = Box::into_raw(words).cast::<usize>();
        payload
    }
}

impl PagableSerialize for Symbol {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        self.as_str().to_owned().pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for Symbol {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let s = String::pagable_deserialize(deserializer)?;
        Ok(Symbol::new(&s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_string_of_any_length_round_trips() {
        for len in [0, 1, 7, 8, 9, 15, 16, 17, 24, 25, 100] {
            let s: String = (0..len).map(|i| (b'a' + i as u8 % 26) as char).collect();
            let symbol = Symbol::new(&s);
            assert_eq!(s, symbol.as_str());
            assert_eq!(len, symbol.len());
            #[allow(clippy::redundant_clone)]
            let cloned = symbol.clone();
            assert_eq!(s, cloned.as_str());
            assert!(symbol == cloned);
            assert!(symbol != Symbol::new(&format!("{s}x")));
            if len > 0 {
                assert!(symbol != Symbol::new(&s[..len - 1]));
            }
            if len <= 16 {
                assert_eq!(name_head(&s), symbol.in_place());
            }
        }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn test_a_symbol_is_four_words() {
        assert_eq!(32, mem::size_of::<Symbol>());
    }
}
