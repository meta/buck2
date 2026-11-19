/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The map data structures used by buck2 and starlark, and the hashing layer they are built on.
//!
//! The hashing layer:
//!
//! - [`FastHasher`], the one hasher these maps use, and [`FastHasherBuilder`].
//! - [`ShortHash`], a 32-bit hash value, small enough to store next to every entry of a map.
//! - [`Hashed<K>`], a key paired with its [`ShortHash`], so a hash computed once can be handed to
//!   lookups and insertions instead of being recomputed at each one.
//!
//! The maps, each with a `*Set` sibling:
//!
//! - [`MiniMutInsertionMap<K, V>`], mutable and insertion-ordered.
//! - [`MiniFixedInsertionMap<K, V>`], immutable and insertion-ordered: the frozen form of the
//!   mutable map, with positions as part of its API.
//! - [`MiniFixedSortedMap<K, V>`], immutable and sorted by key: the frozen form for data whose
//!   arrival order nobody needs, with an iteration order, equality and hash that are functions
//!   of its contents.
//!
//! Every map is one allocation behind a one-word handle, and the fixed insertion map can also be
//! built into memory somebody else owns through its `layout_of`/`init`/view API.
//!
//! # Equality and hashing
//!
//! `==` and `Hash` are order-sensitive on every map and set here, as they are for `Vec`: two
//! insertion-ordered containers are equal when they hold the same entries in the same order, and
//! for [`MiniFixedSortedMap`] that is the same thing as having the same entries, because its order
//! is a function of them. Where the order-insensitive question is the one being asked, it is asked
//! explicitly: [`MiniMutInsertionMap::eq_unordered`].

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
// Hints we disagree with
#![allow(clippy::missing_safety_doc)]
#![cfg_attr(rust_nightly, feature(core_intrinsics))]
#![cfg_attr(rust_nightly, feature(dropck_eyepatch))]
#![cfg_attr(rust_nightly, feature(portable_simd))]
#![cfg_attr(rust_nightly, allow(internal_features))]

mod arrays;
pub mod fixed_insertion_map;
pub mod fixed_insertion_set;
pub mod fixed_sorted_map;
pub mod fixed_sorted_set;
mod fx64;
mod hashed;
mod hasher;
mod hint;
mod insertion_map;
mod iter;
pub mod mut_insertion_map;
pub mod mut_insertion_set;
mod scan;
mod short_hash;

pub use equivalent::Equivalent;

pub use crate::fixed_insertion_map::MiniFixedInsertionMap;
pub use crate::fixed_insertion_set::MiniFixedInsertionSet;
pub use crate::fixed_sorted_map::MiniFixedSortedMap;
pub use crate::fixed_sorted_set::MiniFixedSortedSet;
pub use crate::hashed::Hashed;
pub use crate::hasher::FastHasher;
pub use crate::hasher::FastHasherBuilder;
pub use crate::mut_insertion_map::MiniMutInsertionMap;
pub use crate::mut_insertion_set::MiniMutInsertionSet;
pub use crate::short_hash::ShortHash;
