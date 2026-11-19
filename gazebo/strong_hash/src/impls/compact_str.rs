/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(feature = "compact_str")]

use crate as strong_hash;

crate::impl_strong_hash_for_impl_hash!(compact_str::CompactString);

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;

    use compact_str::CompactString;

    use crate::StrongHash;

    #[test]
    fn test_compact_string_matches_string() {
        for value in [
            "",
            "3030303030303030",
            "this string is too long to be stored inline",
        ] {
            let mut compact_hash = DefaultHasher::new();
            CompactString::new(value).strong_hash(&mut compact_hash);

            let mut string_hash = DefaultHasher::new();
            value.to_owned().strong_hash(&mut string_hash);

            assert_eq!(compact_hash.finish(), string_hash.finish(), "{value:?}");
        }
    }
}
