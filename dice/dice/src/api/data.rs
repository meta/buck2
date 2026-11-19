/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//!
//! Dice allows users to attach immutable data to the engine or to a root. These data are
//! context information for computations to use. These values are NOT tracked by Dice. They are
//! obtained synchronously.
//!
//! To attach data to Dice, you can implement a trait on DiceData, and use the methods to retrieve
//! values itself as the key for setting values.
//! e.g.
//! ```
//! use crate::dice::DiceData;
//!
//! pub trait HasData {
//!     fn my_data(&self) -> usize;
//!
//!     fn other_data(&self) -> &String;
//!
//!     fn set_multi(&mut self, i: usize, s: String);
//! }
//!
//! struct HasDataContainer(usize, String);
//!
//! impl HasData for DiceData {
//!     fn my_data(&self) -> usize {
//!         self.get::<HasDataContainer>().unwrap().0
//!     }
//!
//!     fn other_data(&self) -> &String {
//!         &self.get::<HasDataContainer>().unwrap().1
//!     }
//!
//!     fn set_multi(&mut self, i: usize, s: String) {
//!         self.set(HasDataContainer(i, s));
//!     }
//! }
//!
//! let mut data = DiceData::new();
//! data.set_multi(1, "foo".to_string());
//!
//! assert_eq!(data.other_data(), &"foo".to_string());
//! assert_eq!(data.my_data(), 1);
//! ```

use std::any::Any;
use std::collections::BTreeSet;

use allocative::Allocative;
use anymap3::Map;
use itertools::Itertools;
use thiserror::Error;

#[derive(Error, Debug)]
#[error(
    "should store a value first before requesting a value for requested data key of type `{0}`. Known types are `{1}`"
)]
pub struct MissingData(&'static str, String);

#[derive(Allocative)]
pub struct DiceData(
    #[allocative(skip)] // TODO(nga): measure this.
    Map<dyn Any + Send + Sync>,
    BTreeSet<&'static str>,
);

impl DiceData {
    pub fn new() -> Self {
        Self(Map::new(), BTreeSet::new())
    }

    /// Stores the given data, overriding the previous value if any.
    pub fn set<K: Send + Sync + 'static>(&mut self, val: K) {
        if self.0.insert(val).is_none() {
            self.1.insert(std::any::type_name::<K>());
        }
    }

    pub fn get<K: Send + Sync + 'static>(&self) -> Result<&K, MissingData> {
        self.0
            .get::<K>()
            .ok_or_else(|| MissingData(std::any::type_name::<K>(), self.1.iter().join(", ")))
    }
}
