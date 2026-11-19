/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::BuildHasherDefault;
use std::marker::PhantomData;
use std::sync::Arc;

use allocative::Allocative;
use buck2_sketches::TypedSketch;
use buck2_util::strong_hasher::Blake3StrongHasher;
use derivative::Derivative;
use dupe::Dupe;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use ref_cast::RefCast;
use setsketch::SetSketchParams;
use setsketch::SetSketcher;
use strong_hash::StrongHash;
use strong_hash::UseStrongHashing;

/// Trait for sketch operations, allowing for mocking in tests.
pub(crate) trait Sketcher<T> {
    fn sketch(&mut self, t: &T);
    fn sketch_weighted(&mut self, t: &T, weight: u64);
}

/// This is a struct representing graph sketches returned from DICE call to compute sketches.
/// It satisfies 2 properties.
/// (1) It can be merged with other sketches via VersionedSketcher's `merge` method. It does
/// so by holding directly onto the `SetSketch` type.
/// (2) It implements Dupe, Hash, and Eq. Hash and Eq are implemented by precomputing and holding
/// onto a signature of the sketch.
///
/// The type parameter `S` is a [`TypedSketch`] marker that determines what kind
/// of typed sketch this mergeable sketch represents (e.g. `DependencyGraphSketch`,
/// `MemoryUsageSketch`, `ActionGraphSketch`).  The marker is used by `serialize`
/// to produce a correctly-typed versioned base64 string.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct MergeableGraphSketch<T: StrongHash, S: TypedSketch> {
    version: SketchVersion,
    #[derivative(Debug = "ignore", PartialEq = "ignore", Hash = "ignore")]
    sketch: Arc<S>,
    #[derivative(Debug = "ignore", PartialEq = "ignore", Hash = "ignore")]
    _phantom: PhantomData<fn(T) -> S>,
}

// Manual impl because Allocative derive adds S: Allocative bound
impl<T: StrongHash, S: TypedSketch> Allocative for MergeableGraphSketch<T, S> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visitor.visit_field(allocative::Key::new("version"), &self.version);
        // sketcher field skipped (TODO: implement allocative for SetSketcher)
        visitor.exit();
    }
}

impl<T: StrongHash, S: TypedSketch> Clone for MergeableGraphSketch<T, S> {
    fn clone(&self) -> Self {
        Self {
            version: self.version,
            sketch: self.sketch.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<T: StrongHash, S: TypedSketch> PagableSerialize for MergeableGraphSketch<T, S> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.version.pagable_serialize(serializer)?;
        self.sketch
            .inner()
            .get_registers()
            .pagable_serialize(serializer)?;
        Ok(())
    }
}

impl<'de, T: StrongHash, S: TypedSketch> PagableDeserialize<'de> for MergeableGraphSketch<T, S> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let version = SketchVersion::pagable_deserialize(deserializer)?;
        let registers: Vec<u16> = PagableDeserialize::pagable_deserialize(deserializer)?;
        let sketch = Arc::new(S::from_recommended_registers(registers));
        Ok(Self {
            version,
            sketch,
            _phantom: PhantomData,
        })
    }
}

impl<T: StrongHash, S: TypedSketch> Dupe for MergeableGraphSketch<T, S> {}

impl<T: StrongHash, S: TypedSketch> PartialEq for MergeableGraphSketch<T, S> {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            && self.sketch.inner().get_registers() == other.sketch.inner().get_registers()
    }
}

impl<T: StrongHash, S: TypedSketch> Eq for MergeableGraphSketch<T, S> {}

impl<T: StrongHash, S: TypedSketch> MergeableGraphSketch<T, S> {
    pub(crate) fn new(
        version: SketchVersion,
        sketcher: SetSketcher<UseStrongHashing<T>, Blake3StrongHasher>,
    ) -> Self {
        let sketch = Arc::new(S::from_inner(sketcher.into_sketch()));
        Self {
            version,
            sketch,
            _phantom: PhantomData,
        }
    }

    /// Returns true if the sketch is effectively empty (all registers are zero).
    /// An empty sketch occurs when no items were sketched, and serializes to a
    /// long string of 'A's in base64. Detecting this allows callers to omit the
    /// sketch from output to avoid wasting storage.
    pub fn is_empty(&self) -> bool {
        self.sketch.inner().get_registers().iter().all(|&v| v == 0)
    }

    /// Returns the estimated cardinality of the sketched set.
    /// For unweighted sketches, this is the approximate number of distinct items.
    /// For weighted sketches, this is the approximate sum of weights.
    pub(crate) fn estimated_cardinality(&self) -> f64 {
        self.sketch.inner().cardinality()
    }

    pub fn serialize(&self) -> String {
        self.sketch.to_base64_versioned()
    }
}

#[derive(
    Copy,
    Clone,
    Dupe,
    Debug,
    Eq,
    Hash,
    PartialEq,
    Allocative,
    Pagable,
    derive_more::Display
)]
pub(crate) enum SketchVersion {
    V1,
}

pub(crate) static DEFAULT_SKETCH_VERSION: SketchVersion = SketchVersion::V1;

impl SketchVersion {
    pub(crate) fn create_sketcher<T: StrongHash, S: TypedSketch>(self) -> VersionedSketcher<T, S> {
        let sketcher = match self {
            Self::V1 => SetSketcher::new(
                SetSketchParams::recommended(),
                BuildHasherDefault::<Blake3StrongHasher>::new(), // We want a predictable hash here.
            ),
        };

        VersionedSketcher {
            version: self,
            sketcher,
            _phantom: PhantomData,
        }
    }
}

pub(crate) struct VersionedSketcher<T: StrongHash, S: TypedSketch> {
    version: SketchVersion,
    sketcher: SetSketcher<UseStrongHashing<T>, Blake3StrongHasher>,
    _phantom: PhantomData<fn() -> S>,
}

impl<T: StrongHash, S: TypedSketch> Sketcher<T> for VersionedSketcher<T, S> {
    fn sketch(&mut self, t: &T) {
        self.sketcher.sketch(UseStrongHashing::ref_cast(t));
    }

    fn sketch_weighted(&mut self, t: &T, weight: u64) {
        self.sketcher
            .sketch_weighted(UseStrongHashing::ref_cast(t), weight);
    }
}

impl<T, S: Sketcher<T>> Sketcher<T> for Option<S> {
    fn sketch(&mut self, t: &T) {
        if let Some(s) = self {
            s.sketch(t);
        }
    }

    fn sketch_weighted(&mut self, t: &T, weight: u64) {
        if let Some(s) = self {
            s.sketch_weighted(t, weight);
        }
    }
}

impl<T: StrongHash, S: TypedSketch> VersionedSketcher<T, S> {
    pub(crate) fn into_mergeable_graph_sketch(self) -> MergeableGraphSketch<T, S> {
        MergeableGraphSketch::new(self.version, self.sketcher)
    }

    pub(crate) fn merge(&mut self, other: &MergeableGraphSketch<T, S>) -> buck2_error::Result<()> {
        if self.version == other.version {
            self.sketcher.merge(&other.sketch.inner());
            Ok(())
        } else {
            Err(buck2_error::internal_error!(
                // This is currently an internal error because users cannot specify sketch version to use.
                "Set sketch version mismatch between {} and {}. Cannot merge.",
                self.version,
                other.version
            ))
        }
    }
}
