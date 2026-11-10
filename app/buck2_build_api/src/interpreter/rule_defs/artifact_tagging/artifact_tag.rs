/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_util::strong_hasher::Blake3StrongHasher;
use dupe::Dupe;
use either::Either;
use pagable::Pagable;
use starlark::any::ProvidesStaticType;
use starlark::collections::StarlarkHasher;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::values::Freeze;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::starlark_value;
use strong_hash::StrongHash;

use crate::interpreter::rule_defs::artifact_tagging::StarlarkTaggedCommandLine;
use crate::interpreter::rule_defs::artifact_tagging::StarlarkTaggedValue;
use crate::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;

/// ArtifactTag allows wrapping input and output artifacts in a command line with tags. Those tags
/// will be made visible to artifact visitors. The tags themselves don't have meaning on their own,
/// but they can be compared to each other, which allows grouping inputs and outputs in meaningful
/// categories. This is notably used for dep files to associate inputs tracked by a dep file with
/// the dep file itself.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Dupe,
    Freeze,
    Trace,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    Pagable,
    StarlarkPagable
)]
pub struct ArtifactTag {
    /// Opaque value used only to compare tags for equality. It is serialized
    /// inline, so content-addressed page-out dedup requires it to be byte-stable
    /// across daemon restarts — hence production tags are content-derived (see
    /// [`ArtifactTag::from_identity`]).
    identity: u64,
}

impl ArtifactTag {
    /// For tests only; production code must use [`ArtifactTag::from_identity`].
    pub fn testing_new() -> Self {
        static LAST: AtomicI64 = AtomicI64::new(0);
        let identity = LAST.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let Ok(identity) = identity.try_into() else {
            LAST.fetch_sub(1, Ordering::Relaxed);
            panic!("i64 overflow (should never happen)");
        };
        ArtifactTag { identity }
    }

    /// A tag whose identity is content-derived from the analysis `owner` and a
    /// per-analysis sequence `index`.
    pub fn from_identity(owner: &DeferredHolderKey, index: u64) -> Self {
        let mut hasher = Blake3StrongHasher::new();
        owner.strong_hash(&mut hasher);
        index.strong_hash(&mut hasher);
        ArtifactTag {
            identity: hasher.finish(),
        }
    }
}

impl fmt::Display for ArtifactTag {
    fn fmt(&self, w: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Do not include identity into display because it introduces non-determinism for starlark.
        write!(w, "ArtifactTag(...)")
    }
}

starlark_simple_value!(ArtifactTag);

starlark::methods_static!(ARTIFACT_TAG_METHODS = artifact_tag_methods);

#[starlark_value(type = "ArtifactTag")]
impl<'v> StarlarkValue<'v> for ArtifactTag {
    fn get_methods() -> Option<&'static Methods> {
        Some(ARTIFACT_TAG_METHODS.methods())
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        Ok(match other.downcast_ref::<Self>() {
            Some(other) => self == other,
            None => false,
        })
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        Hash::hash(self, hasher);
        Ok(())
    }
}

/// An `ArtifactTag` is used to associate inputs and outputs in an action.
/// Tags are typically used with dependency files (dep files) to track which inputs are actually used
/// during action execution, enabling more accurate incremental builds.
///
/// For complete documentation including examples and the full workflow, see
/// [`ctx.actions.artifact_tag()`](../AnalysisActions#analysisactionsartifact_tag).
#[starlark_module]
fn artifact_tag_methods(_: &mut MethodsBuilder) {
    /// Tag both input and output artifacts with this tag.
    ///
    /// When artifacts are tagged, artifact visitors (like those used during action execution)
    /// can identify which artifacts belong to which category. This is commonly used with the
    /// `dep_files` parameter in `ctx.actions.run()` to associate inputs with their dependency files.
    ///
    /// Args:
    ///     inner: The artifact(s) or command line arguments. Can be a single artifact,
    ///            a list of artifacts, or cmd_args containing artifacts.
    ///
    /// Returns:
    ///     The tagged value. If `inner` is command-line-like, returns a tagged command line;
    ///     otherwise returns a tagged value.
    fn tag_artifacts<'v>(
        this: &ArtifactTag,
        inner: Value<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<
        Either<
            ValueTyped<'v, StarlarkTaggedValue<'v>>,
            ValueTyped<'v, StarlarkTaggedCommandLine<'v>>,
        >,
    > {
        let value = StarlarkTaggedValue::new(inner, this.dupe());

        Ok(if ValueAsCommandLineLike::unpack_value(inner)?.is_some() {
            Either::Right(heap.alloc_representation(StarlarkTaggedCommandLine::new(value)))
        } else {
            Either::Left(heap.alloc_representation(value))
        })
    }

    /// Tag only input artifacts with this tag (outputs are not tagged).
    ///
    /// This is similar to `tag_artifacts()`, but only applies the tag to input artifacts.
    /// This is useful when you want to track inputs separately from outputs.
    ///
    /// Args:
    ///     inner: The artifact(s) or command line arguments. Can be a single artifact,
    ///            a list of artifacts, or cmd_args containing artifacts.
    ///
    /// Returns:
    ///     The tagged value with tags applied only to inputs. If `inner` is command-line-like,
    ///     returns a tagged command line; otherwise returns a tagged value.
    fn tag_inputs<'v>(
        this: &ArtifactTag,
        inner: Value<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<
        Either<
            ValueTyped<'v, StarlarkTaggedValue<'v>>,
            ValueTyped<'v, StarlarkTaggedCommandLine<'v>>,
        >,
    > {
        let value = StarlarkTaggedValue::inputs_only(inner, this.dupe());

        Ok(if ValueAsCommandLineLike::unpack_value(inner)?.is_some() {
            Either::Right(heap.alloc_representation(StarlarkTaggedCommandLine::new(value)))
        } else {
            Either::Left(heap.alloc_representation(value))
        })
    }
}

#[starlark_module]
#[starlark_types(ArtifactTag as ArtifactTag)]
pub(crate) fn register_artifact_tag(globals: &mut GlobalsBuilder) {}

#[cfg(test)]
mod tests {
    use buck2_core::deferred::key::DeferredHolderKey;

    use super::ArtifactTag;

    #[test]
    fn test_from_identity_is_deterministic_and_distinct() {
        let owner = DeferredHolderKey::testing_new("cell//pkg:target");
        let other = DeferredHolderKey::testing_new("cell//pkg:other");

        // Same (owner, index) must always yield the same identity — this is what
        // content-addressed page-out dedup relies on across daemon processes.
        assert_eq!(
            ArtifactTag::from_identity(&owner, 0),
            ArtifactTag::from_identity(&owner, 0),
        );

        // Distinct indices within one analysis produce distinct tags.
        assert_ne!(
            ArtifactTag::from_identity(&owner, 0),
            ArtifactTag::from_identity(&owner, 1),
        );

        // Distinct owners produce distinct tags even at the same index.
        assert_ne!(
            ArtifactTag::from_identity(&owner, 0),
            ArtifactTag::from_identity(&other, 0),
        );
    }
}
