/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/// The version a transaction runs against: a `(branch, seq)` pair of `dice_core`. Dice runs a
/// single branch today, so `Display` prints `v{seq}`.
pub type VersionNumber = dice_core::Version;
