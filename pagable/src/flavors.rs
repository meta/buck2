/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Custom postcard flavors for pagable serialization that expose position
//! tracking and random access capabilities.
//!
//! These replace postcard's built-in `StdVec` and `Slice` flavors to enable:
//! - `position()`: know how many bytes have been written/read
//! - `write_at()`: patch previously written bytes (serializer)
//! - `seek()`: jump to a different read position (deserializer)

use std::cell::Cell;
use std::rc::Rc;

// ============================================================================
// Serialization flavor
// ============================================================================

/// Replacement for `postcard::ser_flavors::StdVec` that exposes position
/// and supports patching previously written bytes.
pub struct PagableVecFlavor {
    vec: Vec<u8>,
}

impl PagableVecFlavor {
    /// Create a new empty vec flavor.
    pub fn new() -> Self {
        Self { vec: Vec::new() }
    }

    /// Current write position (number of bytes written so far).
    #[inline]
    pub fn position(&self) -> usize {
        self.vec.len()
    }

    /// Overwrite bytes at a previously written position.
    /// Panics if `pos + bytes.len() > position()`.
    #[inline]
    pub fn write_at(&mut self, pos: usize, bytes: &[u8]) {
        self.vec[pos..pos + bytes.len()].copy_from_slice(bytes);
    }
}

impl Default for PagableVecFlavor {
    fn default() -> Self {
        Self::new()
    }
}

impl postcard::ser_flavors::Flavor for PagableVecFlavor {
    type Output = Vec<u8>;

    #[inline(always)]
    fn try_extend(&mut self, data: &[u8]) -> postcard::Result<()> {
        self.vec.extend_from_slice(data);
        Ok(())
    }

    #[inline(always)]
    fn try_push(&mut self, data: u8) -> postcard::Result<()> {
        self.vec.push(data);
        Ok(())
    }

    fn finalize(self) -> postcard::Result<Self::Output> {
        Ok(self.vec)
    }
}

// ============================================================================
// Deserialization flavor
// ============================================================================

/// Shared position tracker for `PagableSlice`.
///
/// Since postcard's `Deserializer` keeps the flavor in a private field,
/// we use `Rc<Cell<usize>>` to share the position between the `PagableSlice`
/// (inside the Deserializer) and the owning `Deserializer` (outside).
#[derive(Clone, Default)]
pub struct SharedPosition(Rc<Cell<usize>>);

impl SharedPosition {
    /// Create a new shared position starting at 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current position.
    #[inline]
    pub fn get(&self) -> usize {
        self.0.get()
    }

    /// Set the current position.
    #[inline]
    pub fn set(&self, pos: usize) {
        self.0.set(pos);
    }
}

/// Replacement for `postcard::de_flavors::Slice` that exposes position
/// and supports seeking to a different read position.
pub struct PagableSlice<'de> {
    bytes: &'de [u8],
    pos: SharedPosition,
}

impl<'de> PagableSlice<'de> {
    /// Create a new slice flavor from a byte slice with a shared position.
    pub fn new(bytes: &'de [u8], pos: SharedPosition) -> Self {
        Self { bytes, pos }
    }
}

impl<'de> postcard::de_flavors::Flavor<'de> for PagableSlice<'de> {
    type Remainder = &'de [u8];
    type Source = &'de [u8];

    #[inline]
    fn pop(&mut self) -> postcard::Result<u8> {
        let p = self.pos.get();
        if p < self.bytes.len() {
            let b = self.bytes[p];
            self.pos.set(p + 1);
            Ok(b)
        } else {
            Err(postcard::Error::DeserializeUnexpectedEnd)
        }
    }

    #[inline]
    fn size_hint(&self) -> Option<usize> {
        Some(self.bytes.len() - self.pos.get())
    }

    #[inline]
    fn try_take_n(&mut self, ct: usize) -> postcard::Result<&'de [u8]> {
        let p = self.pos.get();
        let end = p + ct;
        if end <= self.bytes.len() {
            let slice = &self.bytes[p..end];
            self.pos.set(end);
            Ok(slice)
        } else {
            Err(postcard::Error::DeserializeUnexpectedEnd)
        }
    }

    fn try_take_n_temp<'a>(&'a mut self, ct: usize) -> postcard::Result<&'a [u8]>
    where
        'de: 'a,
    {
        let p = self.pos.get();
        let end = p + ct;
        if end <= self.bytes.len() {
            let slice = &self.bytes[p..end];
            self.pos.set(end);
            Ok(slice)
        } else {
            Err(postcard::Error::DeserializeUnexpectedEnd)
        }
    }

    fn finalize(self) -> postcard::Result<Self::Remainder> {
        Ok(&self.bytes[self.pos.get()..])
    }
}
