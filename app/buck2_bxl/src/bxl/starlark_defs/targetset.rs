/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::ops::Deref;
use std::slice;
use std::sync::OnceLock;

use allocative::Allocative;
use buck2_build_api::actions::query::ActionQueryNode;
use buck2_build_api::interpreter::rule_defs::type_id_domain::Buck2TypeIdDomain;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_query::query::environment::QueryTarget;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use starlark::any::IsStaticType;
use starlark::any::ProvidesStaticType;
use starlark::docs::DocItem;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::pagable::StarlarkDeserialize;
use starlark::pagable::StarlarkDeserializeContext;
use starlark::pagable::StarlarkSerialize;
use starlark::pagable::StarlarkSerializeContext;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::typing::HasTyVTable;
use starlark::typing::Ty;
use starlark::typing::TyBasic;
use starlark::typing::TyStarlarkValue;
use starlark::typing::TyUser;
use starlark::typing::TyUserIndex;
use starlark::typing::TyUserParams;
use starlark::typing::TypingBinOp;
use starlark::values::AllocValue;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueError;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;
use starlark::values::typing::TypeCompiled;
use starlark::values::typing::TypeInstanceId;

use crate::bxl::starlark_defs::alloc_node::AllocNode;

pub(crate) trait NodeLike =
    QueryTarget + std::fmt::Debug + Eq + Dupe + AllocNode + Allocative + Pagable + TargetSetNode;

/// Node kinds a `target_set` can hold.
pub(crate) trait TargetSetNode {
    /// Static type of `StarlarkTargetSet<Self>`, e.g. `bxl.TargetSet[bxl.ConfiguredTargetNode]`.
    fn target_set_ty() -> Ty;
}

impl TargetSetNode for TargetNode {
    fn target_set_ty() -> Ty {
        static TY: OnceLock<Ty> = OnceLock::new();
        TY.get_or_init(new_target_set_ty::<Self>).dupe()
    }
}

impl TargetSetNode for ConfiguredTargetNode {
    fn target_set_ty() -> Ty {
        static TY: OnceLock<Ty> = OnceLock::new();
        TY.get_or_init(new_target_set_ty::<Self>).dupe()
    }
}

impl TargetSetNode for ActionQueryNode {
    fn target_set_ty() -> Ty {
        static TY: OnceLock<Ty> = OnceLock::new();
        TY.get_or_init(new_target_set_ty::<Self>).dupe()
    }
}

/// All node kinds share one generic `StarlarkValue` impl, and with it the `target_set` type
/// name, so the node kind is carried by this `TyUser` instead. That gives each kind a distinct
/// name in signatures and type errors, and types iterating and indexing as the node.
fn new_target_set_ty<Node: NodeLike>() -> Ty
where
    StarlarkTargetSet<Node>: HasTyVTable,
{
    let node_ty = Node::starlark_ty();
    let name = format!("bxl.TargetSet[{node_ty}]");
    let id = TypeInstanceId::from_identity(Buck2TypeIdDomain::BxlTargetSet, &name.as_str());
    Ty::custom(
        TyUser::new(
            name,
            TyStarlarkValue::new::<StarlarkTargetSet<Node>>(),
            id,
            TyUserParams {
                index: Some(TyUserIndex::new(Ty::int(), node_ty.dupe())),
                iter_item: Some(node_ty),
                ..TyUserParams::default()
            },
        )
        .expect("target_set is indexable and iterable"),
    )
}

impl<Node: NodeLike> StarlarkSerialize for StarlarkTargetSet<Node> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> starlark::Result<()> {
        self.pagable_serialize(ctx.pagable())?;
        Ok(())
    }
}

impl<'fv, Node: NodeLike> StarlarkDeserialize<'fv> for StarlarkTargetSet<Node> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> starlark::Result<Self> {
        Ok(Self::pagable_deserialize(ctx.pagable())?)
    }
}

unsafe impl<N: QueryTarget + 'static> starlark::pagable::VtableRegistered for StarlarkTargetSet<N> {}

starlark::register_simple_vtable_entry!(
    StarlarkTargetSet<buck2_node::nodes::unconfigured::TargetNode>
);
starlark::register_simple_vtable_entry!(
    StarlarkTargetSet<buck2_node::nodes::configured::ConfiguredTargetNode>
);
starlark::register_simple_vtable_entry!(
    StarlarkTargetSet<buck2_build_api::actions::query::ActionQueryNode>
);

#[derive(Debug, Display, Clone, Pagable)]
#[derive(NoSerialize, Allocative)] // TODO maybe this should be
/// The StarlarkValue implementation for TargetSet to expose it to starlark.
pub(crate) struct StarlarkTargetSet<Node: QueryTarget>(pub(crate) TargetSet<Node>);

// TODO(nga): derive it.
unsafe impl<'a, Node: QueryTarget + 'static> ProvidesStaticType<'a> for StarlarkTargetSet<Node> {
    type StaticType = Self;
}

impl<Node: QueryTarget + 'static> IsStaticType for StarlarkTargetSet<Node> {
    type Reinfect<'lt> = Self;
}

impl<Node: QueryTarget + AllocNode> StarlarkTargetSet<Node> {
    pub(crate) fn iter<'a, 'v>(&'a self, heap: Heap<'v>) -> impl Iterator<Item = Value<'v>> + 'a
    where
        'v: 'a,
    {
        self.0
            .iter()
            .map(move |target_node| target_node.dupe().alloc(heap))
    }
}

impl<'v, Node: QueryTarget> Freeze<'v> for StarlarkTargetSet<Node> {
    type Frozen<'fv> = StarlarkTargetSet<Node>;

    fn freeze<'fv>(
        self,
        _freezer: &starlark::values::Freezer<'v, 'fv>,
    ) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(self)
    }
}

impl<Node: NodeLike> StarlarkTypeRepr for &StarlarkTargetSet<Node>
where
    StarlarkTargetSet<Node>: HasTyVTable,
{
    type Canonical = Self;

    fn starlark_type_repr() -> Ty {
        StarlarkTargetSet::<Node>::starlark_type_repr()
    }
}

impl<'v, Node: NodeLike> UnpackValue<'v> for &'v StarlarkTargetSet<Node>
where
    StarlarkTargetSet<Node>: HasTyVTable,
{
    type Error = Infallible;

    fn unpack_value_impl(x: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(StarlarkTargetSet::from_value(x))
    }
}

impl<'v, Node: NodeLike> AllocValue<'v> for StarlarkTargetSet<Node>
where
    Self: HasTyVTable,
{
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_simple(self)
    }
}

starlark::methods_static!(STARLARK_TARGET_SET_METHODS = starlark_target_set_methods);

#[starlark_value(type = "target_set")]
impl<'v, Node: NodeLike> StarlarkValue<'v> for StarlarkTargetSet<Node>
where
    Self: HasTyVTable,
{
    fn get_type_starlark_repr() -> Ty {
        Node::target_set_ty()
    }

    fn get_methods() -> Option<&'static Methods> {
        Some(STARLARK_TARGET_SET_METHODS.methods())
    }

    fn bin_op_ty(op: TypingBinOp, rhs: &TyBasic) -> Option<Ty> {
        match op {
            TypingBinOp::Add | TypingBinOp::Sub | TypingBinOp::BitAnd => {
                let set_ty = Node::target_set_ty();
                // Like `add`/`sub`/`bit_and`, require the same node kind. An unknown (`Any`)
                // right-hand side may still be one, so it is allowed.
                let same_kind = matches!(rhs, TyBasic::Any)
                    || set_ty.iter_union() == slice::from_ref(rhs)
                    || *rhs == TyBasic::StarlarkValue(TyStarlarkValue::new::<Self>());
                same_kind.then_some(set_ty)
            }
            _ => None,
        }
    }

    fn iterate_collect(&self, heap: Heap<'v>) -> starlark::Result<Vec<Value<'v>>> {
        Ok(self.iter(heap).collect())
    }

    fn at(&self, index: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        let i = i32::unpack_value_err(index)?;
        if let Ok(i) = usize::try_from(i) {
            if let Some(node) = self.0.get_index(i) {
                return Ok(node.dupe().alloc(heap));
            }
        }
        Err(ValueError::IndexOutOfBound(i).into())
    }

    fn length(&self) -> starlark::Result<i32> {
        self.0.len().try_into().map_err(starlark::Error::new_other)
    }

    fn add(&self, other: Value<'v>, heap: Heap<'v>) -> Option<starlark::Result<Value<'v>>> {
        let other = other.downcast_ref::<Self>()?;
        let union = self.0.union(&other.0);
        Some(Ok(heap.alloc(Self(union))))
    }

    fn sub(&self, other: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        let Some(other) = other.downcast_ref::<Self>() else {
            return ValueError::unsupported_with(self, "-", other);
        };
        let difference = self.0.difference(&other.0)?;
        Ok(heap.alloc(Self(difference)))
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        match other.downcast_ref::<StarlarkTargetSet<Node>>() {
            Some(other) => Ok(self.0 == other.0),
            None => Ok(false),
        }
    }

    fn bit_and(&self, other: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        let Some(other) = other.downcast_ref::<Self>() else {
            return ValueError::unsupported_with(self, "&", other);
        };
        let intersect = self.0.intersect(&other.0)?;
        Ok(heap.alloc(Self(intersect)))
    }
}

// Register each concrete type of TargetSet for HasTyVTable
starlark::register_ty_starlark_value!(StarlarkTargetSet<TargetNode>);
starlark::register_ty_starlark_value!(StarlarkTargetSet<ConfiguredTargetNode>);
starlark::register_ty_starlark_value!(StarlarkTargetSet<ActionQueryNode>);

impl<Node: QueryTarget> From<TargetSet<Node>> for StarlarkTargetSet<Node> {
    fn from(v: TargetSet<Node>) -> Self {
        Self(v)
    }
}

impl<Node: QueryTarget> FromIterator<Node> for StarlarkTargetSet<Node> {
    fn from_iter<Iter: IntoIterator<Item = Node>>(iter: Iter) -> Self {
        let targets = TargetSet::from_iter(iter);
        Self(targets)
    }
}

impl<Node: QueryTarget> Deref for StarlarkTargetSet<Node> {
    type Target = TargetSet<Node>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<Node: NodeLike> StarlarkTargetSet<Node>
where
    Self: HasTyVTable,
{
    pub(crate) fn from_value<'v>(x: Value<'v>) -> Option<&'v Self> {
        x.downcast_ref::<Self>()
    }
}

// Values of `bxl.TargetSet[...]` and the `bxl.*TargetSet` globals. Frozen modules can hold these,
// and pagable serialization can only refer to statics registered by this macro.
starlark::declare_starlark_value_as_type!(pub(crate) UNCONFIGURED_TARGET_SET_TYPE, StarlarkTargetSet<TargetNode>);
starlark::declare_starlark_value_as_type!(pub(crate) CONFIGURED_TARGET_SET_TYPE, StarlarkTargetSet<ConfiguredTargetNode>);
starlark::declare_starlark_value_as_type!(pub(crate) ACTION_QUERY_TARGET_SET_TYPE, StarlarkTargetSet<ActionQueryNode>);

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum TargetSetTypeError {
    #[error(
        "`bxl.TargetSet` expects one of [`bxl.UnconfiguredTargetNode`, `bxl.ConfiguredTargetNode`, `bxl.ActionQueryNode`], got `{0}`"
    )]
    UnsupportedNodeType(Ty),
}

/// The value of `bxl.TargetSet`. As a type it matches a target set of any node kind;
/// `bxl.TargetSet[<node type>]` is the type of target sets of that kind.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("bxl.TargetSet")]
pub(crate) struct StarlarkTargetSetType;

starlark_simple_value!(StarlarkTargetSetType);

impl StarlarkTargetSetType {
    /// Type of a target set of any node kind.
    fn any_kind_ty() -> Ty {
        Ty::unions(vec![
            TargetNode::target_set_ty(),
            ConfiguredTargetNode::target_set_ty(),
            ActionQueryNode::target_set_ty(),
        ])
    }
}

#[starlark_value(type = "bxl.TargetSet")]
impl<'v> StarlarkValue<'v> for StarlarkTargetSetType {
    fn eval_type(&self) -> Option<Ty> {
        Some(Self::any_kind_ty())
    }

    fn at(&self, index: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        let node_ty = TypeCompiled::new(index, heap)?;
        let node_ty = node_ty.as_ty();
        if *node_ty == TargetNode::starlark_ty() {
            Ok(heap.alloc(UNCONFIGURED_TARGET_SET_TYPE))
        } else if *node_ty == ConfiguredTargetNode::starlark_ty() {
            Ok(heap.alloc(CONFIGURED_TARGET_SET_TYPE))
        } else if *node_ty == ActionQueryNode::starlark_ty() {
            Ok(heap.alloc(ACTION_QUERY_TARGET_SET_TYPE))
        } else {
            Err(
                buck2_error::Error::from(TargetSetTypeError::UnsupportedNodeType(node_ty.dupe()))
                    .into(),
            )
        }
    }

    fn documentation(&self) -> DocItem {
        // All node kinds share one methods table, so its docs describe every target set.
        DocItem::Type(
            STARLARK_TARGET_SET_METHODS
                .methods()
                .documentation(Self::any_kind_ty()),
        )
    }
}

/// A set-like object for managing buck2 target nodes.
///
/// It can be obtained from several functions in [`bxl.Context`](../Context) and
/// [`bxl.UqueryContext`](../UqueryContext)/[`bxl.CqueryContext`](../CqueryContext)/[`bxl.AqueryContext`](../AqueryContext),
/// among other places.
///
/// The node kind is part of the type, written `bxl.TargetSet[<node type>]`:
/// * `bxl.TargetSet[bxl.ConfiguredTargetNode]`, also available as [`bxl.ConfiguredTargetSet`](../ConfiguredTargetSet), contains [`ConfiguredTargetNode`](../ConfiguredTargetNode)s
/// * `bxl.TargetSet[bxl.UnconfiguredTargetNode]`, also available as [`bxl.UnconfiguredTargetSet`](../UnconfiguredTargetSet), contains [`UnconfiguredTargetNode`](../UnconfiguredTargetNode)s
/// * `bxl.TargetSet[bxl.ActionQueryNode]` contains [`ActionQueryNode`](../ActionQueryNode)s
///
/// `bxl.TargetSet` on its own matches a target set of any kind. `type()` returns `"target_set"` for all of them.
///
/// It provides common set operations for target nodes.
/// It supports iteration, indexing, addition (union), subtraction (difference), equality comparison, and intersection operations.
///
/// Operations:
/// * `+`  : Union of two TargetSets
/// * `-`  : Difference between two TargetSets
/// * `==` : Equality comparison
/// * `&` : Intersection of two TargetSets
/// * `[]` : Index access
/// * `len()`: Number of targets in set
/// * `iter()`: Iteration over targets
/// * constructor: [`bxl.ctarget_set()`](../#ctarget_set) for `ConfiguredTargetSet` and [`bxl.utarget_set()`](../#utarget_set) for `UnconfiguredTargetSet`
///
/// Example:
/// ```python
/// # Combine sets
/// all_targets = targets1 + targets2  # Union
///
/// # Remove targets
/// remaining = targets1 - targets2    # Difference
///
/// # Check if sets are equal
/// if targets1 == targets2:
///     print("Sets contain same targets")
///
/// # Iterate through targets
/// for target in targets1:
///    print(target)
///
///  # Get target by index
/// first_target = targets1[0]
///
/// # Get number of targets
/// count = len(targets1)
///
/// # Intersection of sets
/// common = targets1 & targets2
/// ```
#[starlark_module]
fn starlark_target_set_methods(builder: &mut MethodsBuilder) {}

starlark::__starlark_pagable_only! {
    #[cfg(test)]
    mod tests {
        use buck2_build_api::actions::registry::RecordedActions;
        use buck2_build_api::analysis::AnalysisResult;
        use buck2_build_api::analysis::registry::RecordedAnalysisValues;
        use buck2_core::configuration::data::ConfigurationData;
        use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
        use buck2_core::deferred::key::DeferredHolderKey;
        use buck2_core::execution_types::execution::ExecutionPlatformResolution;
        use buck2_core::provider::label::ConfiguredProvidersLabel;
        use buck2_core::target::label::label::TargetLabel;
        use buck2_hash::BuckFixedSortedMap;
        use buck2_node::rule_type::RuleType;
        use pagable::PagableDeserialize;
        use pagable::PagableSerialize;
        use starlark::values::FrozenHeapName;
        use starlark::values::OwnedFrozen;

        use super::*;

        #[test]
        fn target_set_types_are_distinct_per_node_kind() {
            let unconfigured = StarlarkTargetSet::<TargetNode>::starlark_type_repr();
            let configured = StarlarkTargetSet::<ConfiguredTargetNode>::starlark_type_repr();
            let action = StarlarkTargetSet::<ActionQueryNode>::starlark_type_repr();

            assert_eq!(unconfigured.to_string(), "bxl.TargetSet[bxl.UnconfiguredTargetNode]");
            assert_eq!(configured.to_string(), "bxl.TargetSet[bxl.ConfiguredTargetNode]");
            assert_eq!(action.to_string(), "bxl.TargetSet[bxl.ActionQueryNode]");
            assert_ne!(unconfigured, configured);

            // Unions must be order-independent and deduplicated; this fails if the kinds sort as
            // equal while comparing unequal.
            assert_eq!(
                Ty::unions(vec![unconfigured.dupe(), configured.dupe(), unconfigured.dupe()]),
                Ty::unions(vec![configured, unconfigured]),
            );
        }

        #[test]
        fn target_set_bin_op_requires_same_kind() {
            type Configured = StarlarkTargetSet<ConfiguredTargetNode>;
            let configured = Configured::starlark_type_repr();
            let unconfigured = StarlarkTargetSet::<TargetNode>::starlark_type_repr();
            let single = |ty: &Ty| ty.iter_union()[0].dupe();
            let bin_op_ty = <Configured as StarlarkValue<'static>>::bin_op_ty;

            assert_eq!(bin_op_ty(TypingBinOp::Add, &single(&configured)), Some(configured.dupe()));
            assert_eq!(bin_op_ty(TypingBinOp::Sub, &TyBasic::Any), Some(configured.dupe()));
            assert_eq!(bin_op_ty(TypingBinOp::BitAnd, &single(&unconfigured)), None);
            assert_eq!(bin_op_ty(TypingBinOp::Add, &single(&Ty::list(Ty::any()))), None);
        }

        #[test]
        fn target_set_ty_round_trips() -> pagable::Result<()> {
            let ty = StarlarkTargetSet::<ConfiguredTargetNode>::starlark_type_repr();

            let mut serializer = pagable::testing::TestingSerializer::new();
            ty.pagable_serialize(&mut serializer)?;
            let bytes = serializer.finish();
            let mut deserializer = pagable::testing::TestingDeserializer::new(&bytes);
            let restored = Ty::pagable_deserialize(&mut deserializer)?;

            assert_eq!(restored, ty);
            assert_eq!(restored.to_string(), ty.to_string());
            Ok(())
        }

        #[test]
        fn configured_target_set_round_trips() -> pagable::Result<()> {
            let node = ConfiguredTargetNode::testing_new(
                TargetLabel::testing_parse("root//pkg:owner")
                    .configure(ConfigurationData::testing_new()),
                "test_rule",
                ExecutionPlatformResolution::new_for_testing(None, Vec::new()),
                vec![],
                None,
            );
            let set = TargetSet::from_iter([node]);
            let owned: OwnedFrozen<Value<'static>> = OwnedFrozen::build(
                FrozenHeapName::user("configured_target_set_round_trips"),
                |heap| heap.alloc_simple(StarlarkTargetSet(set.clone())),
            );

            let mut serializer = pagable::testing::TestingSerializer::new();
            owned.pagable_serialize(&mut serializer)?;
            let bytes = serializer.finish();
            let mut deserializer = pagable::testing::TestingDeserializer::new(&bytes);
            let restored = OwnedFrozen::<Value<'static>>::pagable_deserialize(&mut deserializer)?;

            restored.by_ref(|v| {
                let restored = v
                    .downcast_ref::<StarlarkTargetSet<ConfiguredTargetNode>>()
                    .expect("a target_set");
                assert_eq!(restored.0, set);
            });
            Ok(())
        }

        #[test]
        fn aquery_target_set_fails_page_out_without_panicking() {
            let label = TargetLabel::testing_parse("root//pkg:owner")
                .configure(ConfigurationData::testing_new());
            let analysis = AnalysisResult::new(
                RecordedAnalysisValues::testing_new_actions_only(
                    DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(label.dupe())),
                    RecordedActions::new(0),
                ),
                None,
                BuckFixedSortedMap::default(),
                0,
                0,
                None,
                Vec::new(),
                RuleType::Forward,
            ).expect("test analysis should have valid providers");
            let node = ActionQueryNode::new_analysis(ConfiguredProvidersLabel::default_for(label), analysis);
            let owned: OwnedFrozen<Value<'static>> = OwnedFrozen::build(
                FrozenHeapName::user("aquery_target_set_fails_page_out_without_panicking"),
                |heap| heap.alloc_simple(StarlarkTargetSet(TargetSet::from_iter([node]))),
            );

            let mut serializer = pagable::testing::TestingSerializer::new();
            let err = owned
                .pagable_serialize(&mut serializer)
                .expect_err("aquery target sets are not pagable");
            assert!(
                format!("{err:#}").contains("`ActionQueryNode` cannot be paged out"),
                "{err:#}"
            );
        }
    }
}
