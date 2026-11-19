/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! (Almost) lock-free insertion only hashtable.
//!
//! Lookups are performed without locking (wait-free).
//! Insertions are performed with a shared lock (lock-free unless resize is needed).
//! Resizing is performed with an exclusive lock.
//!
//! Entries are never removed (until the table is dropped).

#![deny(missing_docs)]

pub mod atomic_value;
mod cache_line;
mod fixed_cap;
pub mod raw;
pub mod sharded;
