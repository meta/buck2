/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Type-safe wrappers around [`SetSketch`] that distinguish between different
//! semantic uses of sketches.
//!
//! Three sketch types are currently defined:
//!
//! - [`DependencyGraphSketch`] — the configured dependency graph of a target;
//!   cardinality estimates the number of configured targets.
//! - [`ActionGraphSketch`] — the action graph of a target; cardinality
//!   estimates the number of unique actions.
//! - [`MemoryUsageSketch`] — retained analysis memory of a target; cardinality
//!   estimates the number of bytes.
//!
//! All three share the same underlying [`SetSketch`] data structure and
//! serialization format, but expose domain-specific accessor names so callers
//! don't have to guess what `cardinality()` means in context.
//!
//! The shared behavior is provided by the [`TypedSketch`] trait; a single
//! generic [`Sketch<D>`] wrapper implements it for every domain marker `D`.
//! Domain markers are grouped by category:
//!
//! - **Unweighted** (`DependencyGraph`, `ActionGraph`) — sketches that count
//!   distinct elements inserted via `sketch()`.
//! - **Weighted** (`MemoryUsage`) — sketches whose cardinality represents a
//!   sum of weights inserted via `sketch_weighted()`.

use std::marker::PhantomData;

use setsketch::SetSketch;
use setsketch::SetSketchDecodeError;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Common interface for newtype wrappers around [`SetSketch`].
///
/// Implementors provide access to the inner sketch; the trait supplies
/// constructors, serialization, merge/union, and overlap queries as defaults.
pub trait TypedSketch: Sized + Clone {
    fn from_inner(sketch: SetSketch) -> Self;
    fn inner(&self) -> &SetSketch;
    fn inner_mut(&mut self) -> &mut SetSketch;
    fn into_inner(self) -> SetSketch;

    /// Decode from a base64 string that may or may not have a version prefix.
    fn from_base64(encoded: &str) -> Result<Self, SetSketchDecodeError> {
        SetSketch::from_base64_maybe_versioned(encoded).map(Self::from_inner)
    }

    /// Create from raw register bytes (8192 bytes = 4096 × u16).
    fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        sketch_from_bytes(bytes).map(Self::from_inner)
    }

    /// Serialize to raw register bytes.
    fn to_bytes(&self) -> Vec<u8> {
        sketch_to_bytes(self.inner())
    }

    /// Merge another sketch into this one in place.
    fn merge(&mut self, other: &Self) {
        self.inner_mut().merge(other.inner());
    }

    /// Return a new sketch that is the union of `self` and `other`, consuming `self`.
    fn union(self, other: &Self) -> Self {
        Self::from_inner(self.into_inner().union(other.inner()))
    }

    /// Estimated overlap between two sketches.
    fn absolute_overlap(&self, other: &Self) -> f64 {
        self.inner().absolute_overlap(other.inner())
    }

    /// Confidence interval for the proportion of elements in this sketch
    /// not found in `other`.
    fn approx_proportion_not_included(&self, other: &Self) -> (f64, f64) {
        self.inner().approx_proportion_not_included(other.inner())
    }

    /// Returns the cardinality estimate as an `i64`, truncating toward zero.
    fn get_size_approx(&self) -> i64 {
        self.inner().get_size_approx()
    }

    /// Construct from raw u16 registers using recommended parameters.
    fn from_recommended_registers(registers: Vec<u16>) -> Self {
        Self::from_inner(SetSketch::from_recommended_registers(registers))
    }

    /// Decode from a base64 string with a required version prefix (e.g. `"V1:..."`).
    fn decode_base64_versioned(encoded: &str) -> Result<Self, SetSketchDecodeError> {
        SetSketch::decode_base64_versioned(encoded).map(Self::from_inner)
    }

    /// Encode to a versioned base64 string (e.g. `"V1:<base64_data>"`).
    fn to_base64_versioned(&self) -> String {
        let registers = self.inner().get_registers();
        let mut res = b"V1:".to_vec();
        let mut enc = base64::write::EncoderWriter::new(
            &mut res,
            &base64::engine::general_purpose::STANDARD_NO_PAD,
        );
        for v in registers {
            use std::io::Write;
            enc.write_all(&v.to_ne_bytes()).unwrap();
        }
        enc.finish().unwrap();
        drop(enc);
        String::from_utf8(res).unwrap()
    }
}

// ---------------------------------------------------------------------------
// Generic sketch wrapper
// ---------------------------------------------------------------------------

/// A typed sketch parameterized by a domain marker `D`.
///
/// All domain-specific sketch types ([`DependencyGraphSketch`],
/// [`ActionGraphSketch`], [`MemoryUsageSketch`]) are aliases for this type
/// with different markers.
pub struct Sketch<D>(SetSketch, PhantomData<D>);

impl<D> Clone for Sketch<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<D> std::fmt::Debug for Sketch<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Sketch").field(&self.0).finish()
    }
}

impl<D> Default for Sketch<D> {
    fn default() -> Self {
        Self(SetSketch::default(), PhantomData)
    }
}

impl<D> TypedSketch for Sketch<D> {
    fn from_inner(sketch: SetSketch) -> Self {
        Self(sketch, PhantomData)
    }
    fn inner(&self) -> &SetSketch {
        &self.0
    }
    fn inner_mut(&mut self) -> &mut SetSketch {
        &mut self.0
    }
    fn into_inner(self) -> SetSketch {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Domain category traits
// ---------------------------------------------------------------------------

/// Marker for unweighted sketches that count distinct elements.
pub trait UnweightedDomain {}

/// Marker for weighted sketches whose cardinality represents a sum of weights.
pub trait WeightedDomain {}

impl<D: UnweightedDomain> Sketch<D> {
    /// Estimate the number of distinct elements in the sketch.
    pub fn estimated_count(&self) -> f64 {
        self.0.cardinality()
    }
}

impl<D: WeightedDomain> Sketch<D> {
    /// Estimate the total weight (sum of all weights inserted).
    pub fn estimated_total(&self) -> f64 {
        self.0.cardinality()
    }
}

// ---------------------------------------------------------------------------
// Domain markers
// ---------------------------------------------------------------------------

/// Domain marker for configured dependency graph sketches.
pub struct DependencyGraph;
impl UnweightedDomain for DependencyGraph {}

/// Domain marker for action graph sketches.
pub struct ActionGraph;
impl UnweightedDomain for ActionGraph {}

/// Domain marker for retained analysis memory sketches.
pub struct MemoryUsage;
impl WeightedDomain for MemoryUsage {}

/// Domain marker for artifact size sketches (weighted by file size in bytes).
pub struct ArtifactSize;
impl WeightedDomain for ArtifactSize {}

/// Domain marker for artifact count sketches (unweighted, counting distinct paths).
pub struct ArtifactCount;
impl UnweightedDomain for ArtifactCount {}

// ---------------------------------------------------------------------------
// Public type aliases
// ---------------------------------------------------------------------------

/// A sketch representing the configured dependency graph of a target.
/// Cardinality estimates the number of configured targets in the graph.
pub type DependencyGraphSketch = Sketch<DependencyGraph>;

/// A sketch representing the action graph of a build target.
/// Cardinality estimates the number of unique actions in the graph.
pub type ActionGraphSketch = Sketch<ActionGraph>;

/// A sketch representing the retained analysis memory of a target.
/// Cardinality estimates the number of bytes of retained analysis memory.
pub type MemoryUsageSketch = Sketch<MemoryUsage>;

/// A sketch representing artifact sizes for a build target.
/// Cardinality estimates the total size in bytes of all artifacts.
pub type ArtifactSizeSketch = Sketch<ArtifactSize>;

/// A sketch representing artifact counts for a build target.
/// Cardinality estimates the number of distinct artifact paths.
pub type ArtifactCountSketch = Sketch<ArtifactCount>;

// ---------------------------------------------------------------------------
// Domain-specific accessors
// ---------------------------------------------------------------------------

impl DependencyGraphSketch {
    /// Estimate the number of configured targets in the dependency graph.
    pub fn estimated_target_count(&self) -> f64 {
        self.estimated_count()
    }
}

impl ActionGraphSketch {
    /// Estimate the number of unique actions in the action graph.
    pub fn estimated_action_count(&self) -> f64 {
        self.estimated_count()
    }
}

impl MemoryUsageSketch {
    /// Estimate the number of bytes of retained analysis memory.
    pub fn estimated_bytes(&self) -> f64 {
        self.estimated_total()
    }

    /// Estimate the megabytes of retained analysis memory.
    pub fn estimated_megabytes(&self) -> f64 {
        self.estimated_total() / 1_000_000.0
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Decode a sketch from raw u16 register bytes.
fn sketch_from_bytes(bytes: &[u8]) -> Result<SetSketch, String> {
    if bytes.len() != 8192 {
        return Err(format!("Expected 8192 bytes, got {}", bytes.len()));
    }
    let registers: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| u16::from_ne_bytes(*chunk))
        .collect();
    Ok(SetSketch::from_recommended_registers(registers))
}

/// Serialize a sketch to raw register bytes.
fn sketch_to_bytes(sketch: &SetSketch) -> Vec<u8> {
    sketch
        .get_registers()
        .iter()
        .copied()
        .flat_map(|v| v.to_ne_bytes())
        .collect()
}
