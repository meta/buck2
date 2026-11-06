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
use std::hash::Hasher;
use std::iter;
use std::ptr;
use std::sync::Arc;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_hash::BuckMutSet;
use buck2_util::strong_hasher::Blake3StrongHasher;
use buck2_util::strong_hasher::StrongHash128;
use display_container::display_pair;
use display_container::fmt_container;
use display_container::iter_display_chain;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;
use pagable::pagable_typetag;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::type_matcher;
use starlark::values::Deferred;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::FrozenValueTyped;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::ThinBoxSliceValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::list::AllocList;
use starlark::values::starlark_value;
use starlark::values::typing::TypeInstanceId;
use starlark::values::typing::TypeMatcher;
use starlark::values::typing::TypeMatcherDyn;
use strong_hash::StrongHash;

use crate::actions::impls::json::JsonUnpack;
use crate::actions::impls::json::validate_json;
use crate::actions::impls::json::visit_json_artifacts;
use crate::artifact_groups::ArtifactGroup;
use crate::artifact_groups::TransitiveSetProjectionKey;
use crate::artifact_groups::TransitiveSetProjectionWrapper;
use crate::artifact_groups::deferred::TransitiveSetKey;
use crate::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::InputProperties;
use crate::interpreter::rule_defs::cmd_args::SimpleCommandLineArtifactVisitor;
use crate::interpreter::rule_defs::transitive_set::FrozenTransitiveSetDefinition;
use crate::interpreter::rule_defs::transitive_set::TransitiveSetArgsProjection;
use crate::interpreter::rule_defs::transitive_set::TransitiveSetError;
use crate::interpreter::rule_defs::transitive_set::TransitiveSetJsonProjection;
use crate::interpreter::rule_defs::transitive_set::transitive_set_definition::TransitiveSetDefinitionLike;
use crate::interpreter::rule_defs::transitive_set::transitive_set_definition::TransitiveSetProjectionKind;
use crate::interpreter::rule_defs::transitive_set::transitive_set_iterator::BfsTransitiveSetIterator;
use crate::interpreter::rule_defs::transitive_set::transitive_set_iterator::DfsTransitiveSetIterator;
use crate::interpreter::rule_defs::transitive_set::transitive_set_iterator::PostorderTransitiveSetIterator;
use crate::interpreter::rule_defs::transitive_set::transitive_set_iterator::PreorderTransitiveSetIterator;
use crate::interpreter::rule_defs::transitive_set::transitive_set_iterator::TopologicalTransitiveSetIterator;
use crate::interpreter::rule_defs::transitive_set::transitive_set_iterator::TransitiveSetIteratorLike;
use crate::interpreter::rule_defs::transitive_set::traversal::TransitiveSetOrdering;
use crate::interpreter::rule_defs::transitive_set::traversal::TransitiveSetTraversal;

#[derive(Clone, Debug, Allocative, Pagable)]
#[pagable_typetag(TypeMatcherDyn)]
pub(crate) struct TransitiveSetMatcher {
    pub(crate) type_instance_id: TypeInstanceId,
}

#[type_matcher]
impl TypeMatcher for TransitiveSetMatcher {
    fn matches(&self, value: Value) -> bool {
        let Some(tset) = ValueTyped::<TransitiveSet>::new(value) else {
            return false;
        };
        let exported = &tset.definition.as_ref().exported;
        // Ids, not pointers: page-in can leave two allocations of one logical definition alive.
        exported.set_type_instance_id == self.type_instance_id
    }
}

/// Compact bitfield for per-projection boolean flags, stored as a u64.
#[derive(
    Debug,
    Clone,
    Copy,
    Trace,
    Freeze,
    Allocative,
    PartialEq,
    Eq,
    StarlarkPagable
)]
pub(crate) struct ProjectionBitSet(u64);

impl ProjectionBitSet {
    const MAX_PROJECTIONS: usize = 64;

    pub fn from_bools(bools: &[bool]) -> buck2_error::Result<Self> {
        if bools.len() > Self::MAX_PROJECTIONS {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "TransitiveSet has {} projections, but at most {} are supported",
                bools.len(),
                Self::MAX_PROJECTIONS,
            ));
        }
        let mut bits: u64 = 0;
        for (i, b) in bools.iter().enumerate() {
            if *b {
                bits |= 1u64 << i;
            }
        }
        Ok(Self(bits))
    }

    pub fn get(self, index: usize) -> buck2_error::Result<bool> {
        match self.0.checked_shr(index as u32) {
            Some(shifted) => Ok(shifted & 1 != 0),
            None => Err(internal_error!(
                "Projection index {} out of range (max {})",
                index,
                Self::MAX_PROJECTIONS
            )),
        }
    }
}

#[derive(Debug, Clone, Trace, ProvidesStaticType, Allocative, StarlarkPagable)]
#[repr(C)]
pub struct TransitiveSet<'v> {
    /// A Deferred key that maps back to this set. This is used to compute its inputs.
    #[starlark_pagable(pagable)]
    pub key: TransitiveSetKey,

    /// The TransitiveSetCallable that this set uses.
    pub(crate) definition: FrozenValueTyped<'v, FrozenTransitiveSetDefinition<'v>>,

    /// The immediate value of this node. If None, then this node will not yield anything when
    /// iterated over (but we'll still traverse to its children).
    pub(crate) node: Option<Node<'v>>,

    /// Pre-computed reductions. Those are arbitrary values based on the set's definition.
    pub(crate) reductions: MiniBoxSlice<Value<'v>>,

    pub(crate) projection_path_resolution_may_require_artifact_value: ProjectionBitSet,

    pub(crate) projection_is_eligible_for_dedupe: ProjectionBitSet,

    /// Each projection's hash, see `projection_hash`: one per projection once frozen, none
    /// before.
    #[trace(static)]
    #[starlark_pagable(pagable)]
    projection_hashes: MiniBoxSlice<StrongHash128>,

    /// Read together on first access; `len()` does not read them.
    pub children: Deferred<ThinBoxSliceValue<'v>>,
}

#[derive(Debug, Clone, Trace, Freeze, Allocative, StarlarkPagable)]
#[repr(C)]
pub struct Node<'v> {
    /// The value
    pub value: Value<'v>,

    /// Pre-computed projections.
    pub projections: MiniBoxSlice<Value<'v>>,
}

impl<'v> fmt::Display for TransitiveSet<'v> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt_container(
            f,
            &format!("{}(", self.definition),
            ")",
            iter_display_chain(
                self.node
                    .as_ref()
                    .map(|node| display_pair("value", "=", &node.value)),
                iter::once(format!("{} children", self.children.len())),
            ),
        )
    }
}

impl<'v> Serialize for TransitiveSet<'v> {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = s.serialize_map(Some(3))?;
        s.serialize_entry("definition", &self.definition)?;
        if let Some(node) = self.node.as_ref() {
            s.serialize_entry("value", &node.value)?;
        }

        s.serialize_entry("children", &self.children.len())?;
        s.end()
    }
}

impl<'v> TransitiveSet<'v> {
    pub fn key(&self) -> &TransitiveSetKey {
        &self.key
    }

    /// Compares ids, not pointers: page-in can leave two allocations of one logical frozen module
    /// heap alive, so two definitions of the same `.bzl` variable need not share an address.
    fn matches_definition(
        &self,
        definition: FrozenValueTyped<'v, FrozenTransitiveSetDefinition<'v>>,
    ) -> bool {
        definition.as_ref().exported.set_type_instance_id
            == self.definition.as_ref().exported.set_type_instance_id
    }

    pub fn projection_name(&'v self, projection: usize) -> buck2_error::Result<&'v str> {
        let def = self.definition.as_ref();

        Ok(def
            .operations()
            .projections
            .get_index(projection)
            .internal_error("Invalid projection id")?
            .0
            .as_str())
    }

    pub fn get_projection_value(
        &self,
        projection: usize,
    ) -> buck2_error::Result<Option<Value<'v>>> {
        match &self.node {
            None => Ok(None),
            Some(node) => Ok(Some(
                *node
                    .projections
                    .get(projection)
                    .internal_error("Invalid projection id")?,
            )),
        }
    }

    pub fn get_projection_key(&self, projection: usize) -> TransitiveSetProjectionKey {
        TransitiveSetProjectionKey {
            key: self.key.dupe(),
            projection,
        }
    }

    pub(crate) fn definition(&self) -> FrozenValueTyped<'v, FrozenTransitiveSetDefinition<'v>> {
        self.definition
    }

    pub fn visit_projection_direct_inputs<V: CommandLineArtifactVisitor<'v>>(
        &self,
        projection: usize,
        visitor: &mut V,
    ) -> buck2_error::Result<()> {
        if let Some(projection) = self.get_projection_value(projection)? {
            // It's either an args-like or a json projection. visit_json_artifacts handles both the way we want.
            visit_json_artifacts(projection, visitor)?;
        }
        Ok(())
    }

    pub fn get_projection_sub_inputs(
        &self,
        projection: usize,
    ) -> buck2_error::Result<Vec<ArtifactGroup>> {
        let mut sub_inputs = Vec::new();

        if let Some(projection) = self.get_projection_value(projection)? {
            let mut visitor = SimpleCommandLineArtifactVisitor::new();
            // It's either an args-like or a json projection. visit_json_artifacts handles both the way we want.
            visit_json_artifacts(projection.to_value(), &mut visitor)?;
            sub_inputs.extend(visitor.inputs);
        }

        // Reuse the same projection for children sets.
        for v in self.children.read()?.iter() {
            let v = TransitiveSet::from_value(v.to_value()).internal_error("Invalid deferred")?;
            sub_inputs.push(ArtifactGroup::TransitiveSetProjection(Arc::new(
                TransitiveSetProjectionWrapper::new(
                    TransitiveSetProjectionKey {
                        key: v.key().dupe(),
                        projection,
                    },
                    v.projection_path_resolution_may_require_artifact_value
                        .get(projection)?,
                    v.projection_is_eligible_for_dedupe.get(projection)?,
                ),
            )));
        }
        Ok(sub_inputs)
    }
}

/// Resolves the children of every node reachable from `root`, one level of
/// the graph at a time with each level's resolutions overlapping, so the
/// traversal that follows finds them resident. A level's children, once
/// resident, are what name the next level.
fn prefetch_all_children<'a, 'v: 'a>(root: &'a TransitiveSet<'v>) -> buck2_error::Result<()> {
    let mut seen: BuckMutSet<usize> = BuckMutSet::from_iter([ptr::from_ref(root) as usize]);
    let mut level: Vec<&'a TransitiveSet<'v>> = vec![root];
    while !level.is_empty() {
        Deferred::prefetch_many(level.iter().map(|set| &set.children))?;
        let mut next = Vec::new();
        for set in level {
            for child in set.children.read()?.iter() {
                if let Some(child) = TransitiveSet::from_value(*child)
                    && seen.insert(ptr::from_ref(child) as usize)
                {
                    next.push(child);
                }
            }
        }
        level = next;
    }
    Ok(())
}

impl<'v> TransitiveSet<'v> {
    /// Makes every reachable node's children resident before a traversal that
    /// will visit all of them. A set whose own children are unread was
    /// restored from storage and its descendants' children will be too; one
    /// read per node in turn would serialize the page-ins. Callers that may
    /// stop early should use [`iter`](Self::iter) without this.
    pub fn prefetch_children(&self) -> buck2_error::Result<()> {
        if self.children.peek().is_none() {
            prefetch_all_children(self)?;
        }
        Ok(())
    }

    /// A lazy traversal; reads each node's children when it is reached.
    pub fn iter<'a>(
        &'a self,
        ordering: TransitiveSetOrdering,
    ) -> buck2_error::Result<Box<dyn TransitiveSetIteratorLike<'a, 'v> + 'a>>
    where
        'v: 'a,
    {
        Ok(match ordering {
            TransitiveSetOrdering::Preorder => Box::new(PreorderTransitiveSetIterator::new(self)),
            TransitiveSetOrdering::Postorder => {
                Box::new(PostorderTransitiveSetIterator::new(self)?)
            }
            TransitiveSetOrdering::Topological => {
                Box::new(TopologicalTransitiveSetIterator::new(self)?)
            }
            TransitiveSetOrdering::Bfs => Box::new(BfsTransitiveSetIterator::new(self)),
            TransitiveSetOrdering::Dfs => Box::new(DfsTransitiveSetIterator::new(self)),
        })
    }

    /// Every node's value in `ordering`. Consumes the whole set, so the
    /// children are prefetched first.
    pub fn collect_values(
        &self,
        ordering: TransitiveSetOrdering,
    ) -> buck2_error::Result<Vec<Value<'v>>> {
        self.prefetch_children()?;
        self.iter(ordering)?
            .values()
            .map(|node| node.map(|node| node.value.to_value()))
            .collect()
    }

    /// Every node's `projection` value in `ordering`. Consumes the whole set,
    /// so the children are prefetched first.
    pub(super) fn collect_projection_values(
        &self,
        ordering: TransitiveSetOrdering,
        projection: usize,
    ) -> buck2_error::Result<Vec<Value<'v>>> {
        self.prefetch_children()?;
        self.iter(ordering)?
            .values()
            .map(|node| {
                Ok(node?
                    .projections
                    .get(projection)
                    .internal_error("Invalid projection")?
                    .to_value())
            })
            .collect()
    }
}

impl<'v> Freeze<'v> for TransitiveSet<'v> {
    type Frozen<'fv> = TransitiveSet<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let TransitiveSet {
            key,
            definition,
            node,
            reductions,
            projection_path_resolution_may_require_artifact_value,
            projection_is_eligible_for_dedupe,
            projection_hashes: _,
            children,
        } = self;
        let definition = definition.freeze(freezer)?;
        let node = node.freeze(freezer)?;
        let reductions = reductions.freeze(freezer)?;
        // Freezes the children first, so each has its hashes by the time this set's are
        // computed from them.
        let children = children.freeze(freezer)?;
        let projection_hashes = hash_projections(&key, &definition, node.as_ref(), &children)
            .map_err(|e| FreezeError::new(format!("Hashing the projections of {key}: {e:#}")))?;
        Ok(TransitiveSet {
            key,
            definition,
            node,
            reductions,
            projection_path_resolution_may_require_artifact_value,
            projection_is_eligible_for_dedupe,
            projection_hashes,
            children,
        })
    }
}

/// See `TransitiveSet::projection_hash`. The node's identity is in it because traversal dedups
/// nodes by identity: two sets with equal content but different sharing can render differently.
fn hash_projections<'fv>(
    key: &TransitiveSetKey,
    definition: &FrozenTransitiveSetDefinition<'fv>,
    node: Option<&Node<'fv>>,
    children: &Deferred<ThinBoxSliceValue<'fv>>,
) -> buck2_error::Result<MiniBoxSlice<StrongHash128>> {
    let children: Vec<&TransitiveSet<'fv>> = children
        .read()?
        .iter()
        .map(|child| {
            TransitiveSet::from_value(child.to_value())
                .internal_error("Invalid transitive set child")
        })
        .collect::<buck2_error::Result<_>>()?;
    let def: &dyn TransitiveSetDefinitionLike = definition;
    def.operations()
        .projections
        .values()
        .enumerate()
        .map(|(idx, spec)| {
            let mut hasher = Blake3StrongHasher::new();
            if let TransitiveSetProjectionKind::Args = spec.kind {
                key.strong_hash(&mut hasher);
                hasher.write_usize(children.len());
                for child in &children {
                    child
                        .projection_hash(idx)
                        .internal_error("A child was frozen without its projection hashes")?
                        .strong_hash(&mut hasher);
                }
                match node {
                    None => hasher.write_u8(0),
                    Some(node) => {
                        hasher.write_u8(1);
                        let value = node
                            .projections
                            .get(idx)
                            .internal_error("Invalid projection id")?;
                        TransitiveSetArgsProjection::as_command_line(*value)?.add_to_command_line(
                            &mut CommandLineBuilder::new_hashing(&mut hasher),
                        )?;
                    }
                }
            }
            Ok(hasher.finalize128())
        })
        .collect()
}

starlark_complex_value!(pub TransitiveSet);

starlark::methods_static!(TRANSITIVE_SET_METHODS = transitive_set_methods);

#[starlark_value(type = "TransitiveSet")]
impl<'v> StarlarkValue<'v> for TransitiveSet<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(TRANSITIVE_SET_METHODS.methods())
    }
}

impl<'v> TransitiveSet<'v> {
    pub fn new(
        key: TransitiveSetKey,
        definition: FrozenValueTyped<'v, FrozenTransitiveSetDefinition<'v>>,
        value: Option<Value<'v>>,
        children: impl IntoIterator<Item = Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<Self> {
        let def: &dyn TransitiveSetDefinitionLike = &*definition;
        if !def.has_id() {
            return Err(TransitiveSetError::TransitiveSetUsedBeforeAssignment.into());
        }

        let children = ThinBoxSliceValue::from_iter(children);
        let children_sets = children
            .iter()
            .map(|v| match TransitiveSet::from_value(*v) {
                Some(set) if set.matches_definition(definition) => Ok(set),
                Some(set) => {
                    fn format_def(def: &FrozenTransitiveSetDefinition<'_>) -> String {
                        format!("{:?}", def.as_debug())
                    }
                    Err(TransitiveSetError::TransitiveValueIsOfWrongType {
                        expected: format_def(&definition),
                        got: format_def(&set.definition),
                    })
                }
                None => Err(TransitiveSetError::TransitiveValueIsNotTransitiveSet {
                    got: v.to_string(),
                }),
            })
            .collect::<Result<Vec<_>, _>>()?;

        let node = value
            .map(|value| {
                let projections = def
                    .operations()
                    .projections
                    .iter()
                    .map(|(name, spec)| {
                        let projected_value = eval
                            .eval_function(spec.projection.to_value(), &[value], &[])
                            .map_err(|error| TransitiveSetError::ProjectionError {
                                error: error.into(),
                                name: name.clone(),
                            })?;
                        match spec.kind {
                            TransitiveSetProjectionKind::Args => {
                                TransitiveSetArgsProjection::as_command_line(projected_value)?;
                            }
                            TransitiveSetProjectionKind::Json => {
                                validate_json(JsonUnpack::unpack_value_err(projected_value)?)?;
                            }
                        }
                        buck2_error::Ok(projected_value)
                    })
                    .collect::<Result<MiniBoxSlice<_>, _>>()?;

                buck2_error::Ok(Node { value, projections })
            })
            .transpose()?;

        let reductions =
            def.operations()
                .reductions
                .iter()
                .enumerate()
                .map(|(idx, (name, reduce))| {
                    let children_values = children_sets
                        .iter()
                        .map(|c| {
                            c.reductions.get(idx).copied().with_internal_error(|| {
                                format!("Child {c} is missing reduction {idx}")
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let children_values = eval.heap().alloc(AllocList(children_values));

                    let value = value.unwrap_or_else(Value::new_none);

                    let reduced = eval
                        .eval_function(reduce.to_value(), &[children_values, value], &[])
                        .map_err(|error| TransitiveSetError::ReductionError {
                            error: error.into(),
                            name: name.clone(),
                        })?;

                    buck2_error::Ok(reduced)
                })
                .collect::<Result<MiniBoxSlice<_>, _>>()?;

        let target_platform =
            if let BaseDeferredKey::TargetLabel(configured_label) = key.holder_key().owner() {
                Some(configured_label.cfg())
            } else {
                None
            };

        struct InputVisitor<'a> {
            path_resolution_may_require_artifact_value: bool,
            is_eligible_for_dedupe: bool,
            target_platform: Option<&'a ConfigurationData>,
        }

        impl<'a> InputVisitor<'a> {
            fn new(target_platform: Option<&'a ConfigurationData>) -> Self {
                Self {
                    path_resolution_may_require_artifact_value: false,
                    is_eligible_for_dedupe: true,
                    target_platform,
                }
            }
        }

        impl<'a, 'v> CommandLineArtifactVisitor<'v> for InputVisitor<'a> {
            fn visit_input(
                &mut self,
                input: ArtifactGroup,
                _props: InputProperties,
                _tags: Vec<&ArtifactTag>,
            ) {
                if input.path_resolution_may_require_artifact_value() {
                    self.path_resolution_may_require_artifact_value = true;
                }

                if self.is_eligible_for_dedupe {
                    self.is_eligible_for_dedupe = input
                        .is_eligible_for_dedupe(self.target_platform)
                        == buck2_data::EligibleForDedupe::Eligible;
                }
            }

            fn visit_declared_output(
                &mut self,
                _artifact: OutputArtifact<'v>,
                _tags: Vec<&ArtifactTag>,
            ) {
            }

            fn visit_frozen_output(&mut self, _artifact: Artifact, _tags: Vec<&ArtifactTag>) {}
        }

        let (
            projection_path_resolution_may_require_artifact_value,
            projection_is_eligible_for_dedupe_iter,
        ): (Vec<bool>, Vec<bool>) = def
            .operations()
            .projections
            .iter()
            .enumerate()
            .map(|(idx, (_name, spec))| {
                let mut path_resolution_may_require_artifact_value = false;
                let mut is_eligible_for_dedupe = true;

                if let Some(node) = &node {
                    let projection = node
                        .projections
                        .get(idx)
                        .internal_error("Invalid projection id")?;

                    let mut visitor = InputVisitor::new(target_platform);
                    match spec.kind {
                        TransitiveSetProjectionKind::Args => {
                            TransitiveSetArgsProjection::as_command_line(*projection)?
                                .visit_artifacts(&mut visitor)?;
                        }
                        TransitiveSetProjectionKind::Json => {
                            visit_json_artifacts(*projection, &mut visitor)?
                        }
                    }
                    if visitor.path_resolution_may_require_artifact_value {
                        path_resolution_may_require_artifact_value = true;
                    }
                    if !visitor.is_eligible_for_dedupe {
                        is_eligible_for_dedupe = false;
                    }
                }

                for child in children_sets.iter() {
                    if child
                        .projection_path_resolution_may_require_artifact_value
                        .get(idx)?
                    {
                        path_resolution_may_require_artifact_value = true;
                    }

                    if is_eligible_for_dedupe
                        && !child.projection_is_eligible_for_dedupe.get(idx)?
                    {
                        let is_child_eligible_for_dedupe = child
                            .key
                            .holder_key()
                            .owner()
                            .configured_label()
                            .is_some_and(|l| l.cfg().is_marked_as_exec_platform());
                        if !is_child_eligible_for_dedupe {
                            is_eligible_for_dedupe = false;
                        }
                    }
                }

                Ok::<(bool, bool), buck2_error::Error>((
                    path_resolution_may_require_artifact_value,
                    is_eligible_for_dedupe,
                ))
            })
            .collect::<Result<Vec<(bool, bool)>, _>>()?
            .into_iter()
            .unzip();

        let projection_path_resolution_may_require_artifact_value =
            ProjectionBitSet::from_bools(&projection_path_resolution_may_require_artifact_value)
                .with_buck_error_context(|| {
                    format!("in transitive set {:?}", definition.as_debug())
                })?;
        let projection_is_eligible_for_dedupe =
            ProjectionBitSet::from_bools(&projection_is_eligible_for_dedupe_iter)
                .with_buck_error_context(|| {
                    format!("in transitive set {:?}", definition.as_debug())
                })?;

        Ok(Self {
            key,
            definition,
            node,
            reductions,
            projection_path_resolution_may_require_artifact_value,
            projection_is_eligible_for_dedupe,
            projection_hashes: Default::default(),
            children: Deferred::new(children),
        })
    }

    /// What `TransitiveSetArgsProjection::add_to_command_line` renders for args projection
    /// `projection` of this set, as a hash: every reachable node's identity and its projection's
    /// `CommandLineBuilder::new_hashing` hash, in the structure that any traversal order is a
    /// function of. Computed when the set is frozen; `None` before that, and for json
    /// projections unspecified.
    pub(crate) fn projection_hash(&self, projection: usize) -> Option<StrongHash128> {
        self.projection_hashes.get(projection).copied()
    }

    pub fn new_from_values(
        key: TransitiveSetKey,
        definition: FrozenValueTyped<'v, FrozenTransitiveSetDefinition<'v>>,
        value: Option<Value<'v>>,
        children: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Self> {
        let children = children
            .map(|v| v.iterate(eval.heap()))
            .transpose()?
            .into_iter()
            .flatten();

        Self::new(key, definition, value, children, eval).map_err(Into::into)
    }
}

#[starlark_module]
fn transitive_set_methods(builder: &mut MethodsBuilder) {
    fn project_as_json<'v>(
        this: ValueTyped<'v, TransitiveSet<'v>>,
        projection: &str,
        #[starlark(require = named, default = "preorder")] ordering: &str,
    ) -> starlark::Result<TransitiveSetJsonProjection<'v>> {
        let def = this.definition;

        let index = def
            .operations()
            .get_index_of_projection(TransitiveSetProjectionKind::Json, projection)?;

        Ok(TransitiveSetJsonProjection {
            transitive_set: this,
            projection: index,
            ordering: TransitiveSetOrdering::parse(ordering)?,
        })
    }

    fn project_as_args<'v>(
        this: ValueTyped<'v, TransitiveSet<'v>>,
        projection: &str,
        #[starlark(require = named, default = "preorder")] ordering: &str,
    ) -> starlark::Result<TransitiveSetArgsProjection<'v>> {
        let def = this.definition;

        let index = def
            .operations()
            .get_index_of_projection(TransitiveSetProjectionKind::Args, projection)?;

        Ok(TransitiveSetArgsProjection {
            transitive_set: this,
            projection: index,
            ordering: TransitiveSetOrdering::parse(ordering)?,
        })
    }

    fn reduce<'v>(
        this: ValueTyped<'v, TransitiveSet<'v>>,
        reduction: &str,
    ) -> starlark::Result<Value<'v>> {
        let def = this.definition;

        let index = match def.operations().reductions.get_index_of(reduction) {
            Some(index) => index,
            None => {
                return Err(
                    buck2_error::Error::from(TransitiveSetError::ReductionDoesNotExist {
                        reduction: reduction.into(),
                        valid_reductions: def
                            .operations()
                            .reductions
                            .keys()
                            .map(String::from)
                            .collect::<Vec<_>>(),
                    })
                    .into(),
                );
            }
        };

        Ok(this
            .reductions
            .get(index)
            .copied()
            .with_internal_error(|| format!("Missing reduction {index}"))?)
    }

    fn traverse<'v>(
        this: ValueTyped<'v, TransitiveSet<'v>>,
        #[starlark(require = named, default = "preorder")] ordering: &str,
    ) -> starlark::Result<TransitiveSetTraversal<'v>> {
        Ok(TransitiveSetTraversal {
            inner: this.to_value(),
            ordering: TransitiveSetOrdering::parse(ordering)?,
        })
    }

    #[starlark(attribute)]
    fn definition<'v>(
        this: ValueTyped<'v, TransitiveSet<'v>>,
    ) -> starlark::Result<FrozenValueTyped<'v, FrozenTransitiveSetDefinition<'v>>> {
        Ok(this.definition())
    }

    #[starlark(attribute)]
    fn value<'v>(this: ValueTyped<'v, TransitiveSet<'v>>) -> starlark::Result<Value<'v>> {
        Ok(match this.node.as_ref() {
            Some(node) => node.value,
            None => Value::new_none(),
        })
    }
    #[starlark(attribute)]
    fn children<'v>(this: ValueTyped<'v, TransitiveSet<'v>>) -> starlark::Result<Vec<Value<'v>>> {
        Ok(this.children.read()?.to_vec())
    }
}
