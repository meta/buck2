/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::key;
use crate::key::Key;

/// "Field" describing allocated but unused capacity (e.g. in `Vec`).
pub(crate) const UNUSED_CAPACITY_NAME: Key = key!("unused_capacity");

/// "Field" describing all capacity (e.g. in `Vec`).
pub(crate) const CAPACITY_NAME: Key = key!("capacity");

/// Generic pointee field in types like `Box`.
pub(crate) const PTR_NAME: Key = key!("ptr");

/// Generic name for useful data (e.g. in `Vec`).
pub(crate) const DATA_NAME: Key = key!("data");

pub(crate) const KEY_NAME: Key = key!("key");
pub(crate) const VALUE_NAME: Key = key!("value");
