/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Cycle detection in DICE

use std::any::Any;
use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use allocative::Allocative;
use cmp_any::PartialEqAny;

/// A `Key` that has been requested within Dice.
pub trait RequestedKey: Allocative + Display + Debug + Send + Sync {
    fn get_key_equality(&self) -> PartialEqAny<'_>;
    fn hash(&self, state: &mut dyn Hasher);
    /// The key as DICE handed it over, for downcasting to its concrete type.
    fn as_any(&self) -> &dyn Any;
}

impl<T> RequestedKey for T
where
    T: Allocative + Display + Debug + Hash + Eq + Send + Sync + 'static,
{
    fn get_key_equality(&self) -> PartialEqAny<'_> {
        PartialEqAny::new(self)
    }

    fn hash(&self, mut state: &mut dyn Hasher) {
        self.hash(&mut state)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A dependency cycle: each key depends on the next, and the last on the first.
#[derive(Debug, Allocative)]
pub struct CycleError {
    pub keys: Vec<Arc<dyn RequestedKey>>,
}

impl Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Dependency cycle detected (`->` means \"depends on\"):")?;
        for k in &self.keys {
            writeln!(f, "  {k} ->")?;
        }
        // Point back at the first key, closing the cycle.
        write!(f, "  {}", self.keys[0])
    }
}

impl std::error::Error for CycleError {}

impl Hash for dyn RequestedKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash(state)
    }
}

impl PartialEq for dyn RequestedKey {
    fn eq(&self, other: &Self) -> bool {
        self.get_key_equality() == other.get_key_equality()
    }
}

impl Eq for dyn RequestedKey {}
