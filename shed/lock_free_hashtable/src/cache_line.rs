/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Cache-line isolation for fields with conflicting access patterns.

/// Puts `T` on its own 128-byte block so that writes to neighboring fields do
/// not invalidate the cache line holding `T`. 128 bytes covers a 64-byte line
/// plus its adjacent-line prefetch buddy.
#[repr(align(128))]
pub(crate) struct IsolatedCacheLine<T>(pub(crate) T);
