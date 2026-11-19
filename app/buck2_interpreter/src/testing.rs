/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use pagable::Pagable;
use starlark::values::FrozenHeapName;
use strong_hash::StrongHash;

/// Testing sentinel for buck2 test code.
/// Used as `FrozenHeapName::User(Box::new(Buck2TestHeapName))`.
#[derive(Clone, derive_more::Display, Debug, Hash, StrongHash, Pagable)]
#[pagable::pagable_typetag(starlark::values::UserHeapName)]
#[display("Buck2TestHeapName")]
pub struct Buck2TestHeapName;

impl Buck2TestHeapName {
    pub fn frozen_heap_name() -> FrozenHeapName {
        FrozenHeapName::User(Box::new(Self))
    }
}
