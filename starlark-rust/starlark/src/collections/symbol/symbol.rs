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
use starlark_derive::Trace;
use starlark_map::Hashed;
use starlark_map::StarlarkHashValue;

use crate as starlark;
use crate::collections::aligned_padded_str::AlignedPaddedStr;

/// A pre-hashed string used for efficient dictionary lookup.
#[derive(
    Clone,
    Trace,
    Allocative,
    ProvidesStaticType,
    StarlarkPagableViaPagable
)]
pub(crate) struct Symbol {
    hash: u64,
    len: u32,
    payload: Box<[usize]>,
    small_hash: StarlarkHashValue,
    /// The first sixteen bytes of the string, as [`name_head`] gives them. They are what a
    /// methods table compares a name by, so that a lookup does not follow `payload`.
    head: [u64; 2],
}

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
    fn eq(&self, other: &Self) -> bool {
        if self.len != other.len {
            return false;
        }

        let p1 = &*self.payload;
        let p2 = &*other.payload;
        // Important to use the payload len, which is in u64 units, rather than len which is in u8
        for i in 0..self.payload.len() {
            // Safe because we checked the lengths at the start
            if unsafe { p1.get_unchecked(i) != p2.get_unchecked(i) } {
                return false;
            }
        }
        true
    }
}

impl Eq for Symbol {}

impl Symbol {
    pub(crate) fn new(x: &str) -> Self {
        Self::new_hashed(Hashed::new(x))
    }

    pub(crate) fn new_hashed(x: Hashed<&str>) -> Self {
        let small_hash = x.hash();
        let hash = small_hash.promote();
        let len = x.key().len();
        let len_words = len.div_ceil(mem::size_of::<usize>());
        let mut payload = vec![0; len_words]; // 0 pad it at the end
        unsafe {
            copy_nonoverlapping(x.key().as_ptr(), payload.as_mut_ptr() as *mut u8, len);
        }
        Self {
            hash,
            len: len.try_into().unwrap(),
            payload: payload.into_boxed_slice(),
            small_hash,
            head: name_head(x.key()),
        }
    }

    #[inline]
    pub(crate) fn hash(&self) -> u64 {
        self.hash
    }

    pub(crate) fn as_str(&self) -> &str {
        // All safe because we promise we started out with a str
        unsafe {
            let s = slice::from_raw_parts(self.payload.as_ptr() as *const u8, self.len as usize);
            str::from_utf8_unchecked(s)
        }
    }

    #[inline]
    pub(crate) fn as_aligned_padded_str(&self) -> AlignedPaddedStr<'_> {
        unsafe { AlignedPaddedStr::new(self.len as usize, self.payload.as_ptr()) }
    }

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

    /// The first sixteen bytes of the string: see [`name_head`].
    #[inline]
    pub(crate) fn head(&self) -> [u64; 2] {
        self.head
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
