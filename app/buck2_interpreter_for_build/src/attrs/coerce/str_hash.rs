/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use twox_hash::XxHash3_64;

/// Unspecified fast hash for short strings.
pub(crate) fn str_hash(s: &str) -> u64 {
    XxHash3_64::oneshot(s.as_bytes())
}
