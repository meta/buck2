/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Reconstructable handles to a [`PagableDeserializer`].
//!
//! A [`PagableDeserializerRecipe`] owns enough state to build a fresh
//! deserializer on demand. Stash one and call `open()`
//! Useful for partial-heap deserialization that keeps many bytes-blobs
//! openable without holding their deserializers live.

use std::sync::Arc;

use allocative::Allocative;

use crate::PagableDeserializer;
use crate::context::PagableDeserializerImpl;
use crate::storage::data::PagableData;
use crate::storage::handle::PagableStorageHandle;

/// Reconstructable handle to a [`PagableDeserializer`].
///
/// `Send + Sync`; cheap to share (typically behind `Arc`).
pub trait PagableDeserializerRecipe: Allocative + Send + Sync {
    /// Build a fresh deserializer positioned at the start of this recipe's data.
    ///
    /// Storage is supplied by the active owner because storage-lifetime state
    /// may itself retain a recipe, and capturing storage here would create an
    /// ownership cycle.
    fn open<'a>(
        &'a self,
        storage: &'a PagableStorageHandle,
    ) -> Box<dyn PagableDeserializer<'a> + 'a>;

    /// Serialized payload bytes this recipe keeps alive so it can be reopened
    /// — the standing memory cost of being able to page a value in lazily.
    /// The retained arc list is not counted.
    fn retained_data_len(&self) -> usize;
}

static_assertions::assert_obj_safe!(PagableDeserializerRecipe);

/// [`PagableDeserializerRecipe`] for
/// [`PagableDeserializerImpl`]: owns
/// its bytes via `Arc<PagableData>`.
#[derive(Allocative, Clone)]
pub struct PagableDeserializerRecipeImpl {
    data: Arc<PagableData>,
}

impl PagableDeserializerRecipeImpl {
    pub(crate) fn new(data: Arc<PagableData>) -> Self {
        Self { data }
    }

    pub fn data(&self) -> &Arc<PagableData> {
        &self.data
    }
}

impl PagableDeserializerRecipe for PagableDeserializerRecipeImpl {
    fn open<'a>(
        &'a self,
        storage: &'a PagableStorageHandle,
    ) -> Box<dyn PagableDeserializer<'a> + 'a> {
        Box::new(PagableDeserializerImpl::new(&self.data, storage))
    }

    fn retained_data_len(&self) -> usize {
        self.data.data.len()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde::Deserialize;
    use serde::Serialize;

    use super::*;
    use crate::context::PagableSerializerImpl;
    use crate::storage::data::PagableData;
    use crate::storage::handle::PagableStorageHandle;
    use crate::storage::in_memory::InMemoryPagableStorage;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct Trio {
        a: u32,
        b: u32,
        c: u32,
    }

    fn make_storage() -> PagableStorageHandle {
        let storage = InMemoryPagableStorage::new();
        PagableStorageHandle::new(storage.handle())
    }

    fn make_data(values: &Trio) -> Arc<PagableData> {
        let mut ser = PagableSerializerImpl::testing_new();
        values
            .serialize(crate::PagableSerializer::serde(&mut ser))
            .unwrap();
        let finished = ser.finish().unwrap();
        Arc::new(PagableData {
            data: finished.data,
            arcs: vec![],
        })
    }

    #[test]
    fn open_at_default_reads_first_field() {
        let trio = Trio {
            a: 11,
            b: 22,
            c: 33,
        };
        let data = make_data(&trio);
        let storage = make_storage();
        let recipe = PagableDeserializerRecipeImpl::new(data);

        let mut de = recipe.open(&storage);
        let a: u32 = u32::deserialize(de.serde()).unwrap();
        assert_eq!(a, 11);
    }

    #[test]
    fn two_opens_on_same_recipe_are_independent() {
        let trio = Trio { a: 1, b: 2, c: 3 };
        let data = make_data(&trio);
        let storage = make_storage();
        let recipe = PagableDeserializerRecipeImpl::new(data);

        let mut d1 = recipe.open(&storage);
        let mut d2 = recipe.open(&storage);

        let v1: u32 = u32::deserialize(d1.serde()).unwrap();
        let v2: u32 = u32::deserialize(d2.serde()).unwrap();

        assert_eq!(v1, 1);
        assert_eq!(v2, 1);
    }

    #[test]
    fn boxed_as_trait_object() {
        let trio = Trio { a: 7, b: 8, c: 9 };
        let storage = make_storage();
        let data = make_data(&trio);
        let recipe: Box<dyn PagableDeserializerRecipe> =
            Box::new(PagableDeserializerRecipeImpl::new(data));

        let mut de = recipe.open(&storage);
        let v: u32 = u32::deserialize(de.serde()).unwrap();
        assert_eq!(v, 7);
    }
}
