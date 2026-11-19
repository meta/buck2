/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! An interned `ForwardRelativePath` type.
//!
//! - `base`: The `IForwardRelativePath` handle, its encoding, and the core API that every
//!   invariant depends on
//! - `interner`: The lock-free table of interned paths and the hashing it uses
//! - `bump`: The sharded bump arena the interned bytes live in
//! - `impls`: Trait implementations and path methods built on the base API

mod base;
mod bump;
mod impls;
mod interner;

pub use base::IForwardRelativePath;

/// Bytes the interner currently holds from the global allocator, including unused space in its
/// arena chunks. Intended for profiling.
pub fn get_forward_rel_path_interner_bytes() -> usize {
    interner::INTERNER.bytes_allocated()
}
