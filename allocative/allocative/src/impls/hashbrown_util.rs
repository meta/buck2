/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::mem;

/// Approximate the number of buckets hashbrown allocates for the given capacity.
///
/// `capacity` is the value returned by `HashMap::capacity()`, i.e. the number
/// of elements that can be inserted without reallocating. Hashbrown uses a 7/8
/// load factor, so the actual bucket count is larger than the usable capacity.
///
/// Exact for freshly allocated tables. May underestimate when tombstone
/// deletions have reduced the value returned by `HashMap::capacity()` below
/// the 7/8 load-factor boundary for the actual bucket count.
pub fn bucket_count_for_capacity(capacity: usize) -> usize {
    if capacity == 0 {
        0
    } else if capacity < 4 {
        4
    } else if capacity < 8 {
        8
    } else {
        // hashbrown uses checked_mul here; saturating is sufficient for an estimate
        // since a capacity large enough to overflow * 8 can't exist in practice.
        (capacity.saturating_mul(8) / 7).next_power_of_two()
    }
}

/// Approximate allocated memory for hashbrown `RawTable`. See docs for
/// `bucket_count_for_capacity` on how incorrect this estimate may be.
pub fn raw_table_alloc_size_for_capacity<T>(capacity: usize) -> usize {
    let buckets = bucket_count_for_capacity(capacity);
    let size_of_control_byte = 1;
    (mem::size_of::<T>() + size_of_control_byte) * buckets
}
