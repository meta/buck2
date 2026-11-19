/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use buck2_core::plugins::PluginKind;
#[allow(unused_imports)]
use buck2_hash::BuckHasher;
use pagable::Pagable;
use static_interner::interner;

use crate::attrs::spec::AttributeId;
use crate::attrs::spec::AttributeSpec;
use crate::nodes::unconfigured::RuleKind;
use crate::rule_type::RuleType;

#[derive(Debug, Eq, PartialEq, Hash, Pagable, Allocative, Clone, dupe::Dupe)]
pub enum RuleIncomingTransition {
    None,
    Fixed(Arc<TransitionId>),
    /// This rule has an `incoming_transition` attribute
    FromAttribute,
}

/// Common rule data needed in `TargetNode`.
#[derive(Debug, Eq, PartialEq, Hash, Pagable, Allocative)]
pub struct Rule {
    /// The attribute spec. This holds the attribute name -> index mapping and the default values
    /// (for those attributes without explicit values).
    pub attributes: AttributeSpec,
    /// The 'type', used to find the implementation function from the graph
    pub rule_type: RuleType,
    /// The kind of rule, e.g. configuration or otherwise.
    pub rule_kind: RuleKind,
    /// Transition to apply to the target.
    pub cfg: RuleIncomingTransition,
    /// The plugin kinds that are used by the target
    pub uses_plugins: Vec<PluginKind>,
    /// The named execution groups the rule declares, in declaration order.
    pub execution_groups: Vec<RuleExecutionGroup>,
    /// Whether the rule may use the default execution group: true unless it declares named
    /// execution groups without `DefaultExecGroup`.
    pub default_execution_group: bool,
    /// The attribute created by `attrs.exec_compatible_with(exec_group =
    /// DefaultExecGroup)`, whose constraints only the default group's execution platform must
    /// satisfy.
    pub default_exec_compatible_with_attr: Option<AttributeId>,
}

/// A named execution group a rule declares.
#[derive(Debug, Eq, PartialEq, Hash, Pagable, Allocative)]
pub struct RuleExecutionGroup {
    pub group: NamedExecutionGroup,
    /// The attribute created by `attrs.exec_compatible_with(exec_group = ...)` for this
    /// group, whose constraints the group's execution platform must satisfy.
    pub exec_compatible_with_attr: Option<AttributeId>,
}

interner!(INTERNER, BuckHasher, Rule);
