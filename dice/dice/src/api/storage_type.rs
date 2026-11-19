/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use dupe::Dupe;

/// Storage type for a cached entry.
///
/// For an Injected entry, we must store all values that are still reachable because we
/// cannot recompute them.
#[derive(Debug, Clone, Copy, Dupe, Allocative)]
pub enum StorageType {
    Normal,
    Injected,
}
