/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod buck2_sketches;
pub mod dice;
pub mod events;
mod implementation;
pub mod types;

pub type FxMultiMap<K, V> = multimap::MultiMap<K, V, buck2_hash::BuckHasherBuilder>;

pub mod testing {
    pub use super::implementation::traverse::traverse_partial_action_graph;
    pub use super::implementation::traverse::traverse_target_graph;
}
