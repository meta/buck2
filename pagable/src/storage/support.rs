/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use postcard::ser_flavors::Flavor;

use crate::PagableSerializer;
use crate::arc_erase::ArcEraseDyn;
use crate::traits::PagableCursor;
use crate::traits::StorageContext;

/// Serializer used during the paging process to serialize arcs and their nested dependencies.
///
/// This serializer collects both the serialized data and references to nested arcs,
/// enabling recursive serialization where nested arcs are tracked separately for
/// content-addressable storage.
pub struct SerializerForPaging<'a> {
    serde: postcard::Serializer<crate::flavors::PagableVecFlavor>,
    arcs: Vec<Box<dyn ArcEraseDyn>>,
    storage_context: &'a StorageContext,
}

impl<'a> SerializerForPaging<'a> {
    pub fn new(storage_context: &'a StorageContext) -> Self {
        Self {
            serde: postcard::Serializer {
                output: crate::flavors::PagableVecFlavor::new(),
            },
            arcs: Vec::new(),
            storage_context,
        }
    }

    /// Returns the serialized data and collected nested arcs.
    ///
    /// Consumes the serializer and returns a tuple of (serialized bytes, nested arcs).
    pub fn finish(self) -> (Vec<u8>, Vec<Box<dyn ArcEraseDyn>>) {
        (self.serde.output.finalize().unwrap(), self.arcs)
    }
}

impl PagableSerializer for SerializerForPaging<'_> {
    fn serde(&mut self) -> &mut postcard::Serializer<crate::flavors::PagableVecFlavor> {
        &mut self.serde
    }

    fn serialize_arc(&mut self, arc: &dyn ArcEraseDyn) -> crate::Result<()> {
        self.arcs.push(arc.clone_dyn());
        Ok(())
    }

    fn position(&mut self) -> PagableCursor {
        PagableCursor {
            byte_pos: self.serde.output.position(),
            arc_index: self.arcs.len(),
        }
    }

    fn storage_context(&self) -> &StorageContext {
        self.storage_context
    }
}
