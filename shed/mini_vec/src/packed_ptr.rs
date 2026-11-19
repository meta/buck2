/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Provides [`PackedPtr`].

mod def;
mod impls;
mod ptr_value;
mod repr;

#[cfg(test)]
mod tests;

pub use def::PackedPtr;
pub use ptr_value::PackedStorageOwned;
pub use ptr_value::PackedStorageRef;
pub use ptr_value::PointerValue;
pub use ptr_value::PointerValueAllocated;
