/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::iter;
use std::sync::Arc;
use std::sync::LazyLock;

use allocative::Allocative;
use buck2_core::build_file_path::BuildFilePath;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::configuration::pair::ConfigurationNoExec;
use buck2_core::configuration::transition::applied::TransitionApplied;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::execution_types::execution::ExecutionPlatform;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::package::source_path::SourcePathRef;
use buck2_core::plugins::PluginKind;
use buck2_core::plugins::PluginKindSet;
use buck2_core::plugins::PluginLists;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_util::arc_str::ArcStr;
use dupe::Dupe;
use either::Either;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;
use starlark_map::Hashed;
use strong_hash::StrongHash;

use crate::attrs::arena::OwnedCoercedAttr;
use crate::attrs::attr::Attribute;
use crate::attrs::attr_type::AttrType;
use crate::attrs::attr_type::any_matches::AnyMatches;
use crate::attrs::attr_type::dep::DepAttr;
use crate::attrs::attr_type::dep::DepAttrTransition;
use crate::attrs::attr_type::dep::DepAttrType;
use crate::attrs::attr_type::query::ResolvedQueryLiterals;
use crate::attrs::attr_type::string::StringLiteral;
use crate::attrs::coerced::CoercedAttrRefKind;
use crate::attrs::configuration_context::AttrConfigurationContextImpl;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::configured_attr_full::ConfiguredAttrFull;
use crate::attrs::configured_traversal::ConfiguredAttrTraversal;
use crate::attrs::inspect_options::AttrInspectOptions;
use crate::attrs::spec::internal::TESTS_ATTRIBUTE;
use crate::attrs::strong_hash_without_config::StrongHashWithoutConfig;
use crate::bzl_or_bxl_path::BzlOrBxlPath;
use crate::call_stack::StarlarkCallStack;
use crate::call_stack::StarlarkTargetCallStackRoot;
use crate::configuration::resolved::ConfigurationSettingKey;
use crate::configuration::resolved::MatchedConfigurationSettingKeys;
use crate::configuration::resolved::MatchedConfigurationSettingKeysWithCfg;
use crate::nodes::attributes::DEPS;
use crate::nodes::attributes::EXECUTION_GROUP_PLATFORMS;
use crate::nodes::attributes::EXECUTION_PLATFORM;
use crate::nodes::attributes::ONCALL;
use crate::nodes::attributes::PACKAGE;
use crate::nodes::attributes::PLUGINS;
use crate::nodes::attributes::TARGET_CONFIGURATION;
use crate::nodes::attributes::TYPE;
use crate::nodes::unconfigured::RuleKind;
use crate::nodes::unconfigured::TargetNode;
use crate::provider_id_set::ProviderIdSet;
use crate::rule_type::RuleType;
use crate::rule_type::StarlarkRuleType;

/// ConfiguredTargetNode contains the information for a target in a particular configuration.
///
/// Most information (like attribute values) is constructed when requested and not stored
/// in the node, instead the node just stores the base TargetNode and a configuration for
/// resolving the attributes. This saves memory, but users should try avoid repeatedly
/// requesting the same information.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Allocative, Pagable)]
pub struct ConfiguredTargetNode(triomphe::Arc<Hashed<ConfiguredTargetNodeData>>);

// SAFETY: `_assert_send_sync` requires the only field to be `Send + Sync`, so these
// impls hold exactly when the derived ones would. Spelled out so that auto-trait
// proofs for the many futures holding nodes stop here instead of walking the node
// and attribute representation in every crate; see `dice::DiceComputations`.
unsafe impl Send for ConfiguredTargetNode {}
unsafe impl Sync for ConfiguredTargetNode {}

fn _assert_send_sync(ConfiguredTargetNode(inner): &ConfiguredTargetNode) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(inner);
}

impl Dupe for ConfiguredTargetNode {}

#[derive(Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
enum TargetNodeOrForward {
    TargetNode(TargetNode),
    /// The node this one forwards to. A forward node's only attribute is a
    /// configured dep on that node, served from its label.
    Forward(ConfiguredTargetNode),
}

const ACTUAL_ATTR_NAME: &str = "actual";

/// The forward node's `actual` attribute, configured: a dep on `forwarded`.
fn actual_attr(forwarded: &ConfiguredTargetNode) -> ConfiguredAttrFull<'static> {
    let label = forwarded.label();
    ConfiguredAttrFull {
        name: ACTUAL_ATTR_NAME,
        attr: ConfiguredTargetNode::actual_attribute(),
        value: ConfiguredAttr::Dep(Box::new(DepAttr {
            attr_type: DepAttrType::new(
                ProviderIdSet::EMPTY,
                DepAttrTransition::Identity(PluginKindSet::EMPTY),
            ),
            label: ProvidersLabel::default_for(label.unconfigured().dupe())
                .configure(label.cfg().dupe()),
        })),
    }
}

impl TargetNodeOrForward {
    fn rule_type(&self) -> &RuleType {
        match self {
            TargetNodeOrForward::TargetNode(target_node) => target_node.rule_type(),
            TargetNodeOrForward::Forward(..) => &RuleType::Forward,
        }
    }

    fn underlying_rule_type(&self) -> &RuleType {
        match self {
            TargetNodeOrForward::TargetNode(target_node) => target_node.rule_type(),
            TargetNodeOrForward::Forward(node) => node.underlying_rule_type(),
        }
    }

    fn rule_kind(&self) -> RuleKind {
        match self {
            TargetNodeOrForward::TargetNode(x) => x.rule_kind(),
            TargetNodeOrForward::Forward(x) => x.rule_kind(),
        }
    }

    fn buildfile_path(&self) -> &BuildFilePath {
        match self {
            TargetNodeOrForward::TargetNode(target_node) => target_node.buildfile_path(),
            TargetNodeOrForward::Forward(x) => x.buildfile_path(),
        }
    }

    fn is_visible_to(&self, target: TargetLabel) -> buck2_error::Result<bool> {
        match self {
            TargetNodeOrForward::TargetNode(node) => node.is_visible_to(&target),
            TargetNodeOrForward::Forward(forward) => forward.is_visible_to(&target),
        }
    }

    fn not_visible_to_error(&self, consumer: TargetLabel) -> crate::visibility::VisibilityError {
        match self {
            TargetNodeOrForward::TargetNode(node) => node.not_visible_to_error(consumer),
            TargetNodeOrForward::Forward(forward) => forward.not_visible_to_error(consumer),
        }
    }

    fn oncall(&self) -> Option<&str> {
        match self {
            TargetNodeOrForward::TargetNode(node) => node.oncall(),
            TargetNodeOrForward::Forward(forward) => forward.oncall(),
        }
    }

    fn unmatched_select_incompatible(&self) -> Option<&ArcStr> {
        match self {
            TargetNodeOrForward::TargetNode(node) => node.unmatched_select_incompatible(),
            // The only attribute of a forward node is `actual`, which has no select.
            TargetNodeOrForward::Forward(..) => None,
        }
    }
}

// TODO(cjhopman): There's a lot of optimization opportunities here.
//  1. we iterate over and configure the attributes multiple times, that could be improved in a bunch of ways
//  2. we store the same resolvedconfiguration probably in a bunch of nodes, that could be made smaller or shared
//  3. deps could probably be approximated a diff against the targetnode's deps
#[derive(Eq, PartialEq, Hash, Allocative, Pagable)]
struct ConfiguredTargetNodeData {
    label: Hashed<ConfiguredTargetLabel>,
    target_node: TargetNodeOrForward,
    resolved_configuration: MatchedConfigurationSettingKeysWithCfg,
    resolved_transition_configurations:
        BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>,
    execution_platform_resolution: ExecutionPlatformResolution,
    // all_deps includes regular deps and transitioned deps,
    // and includes exec deps and configuration deps.
    // TODO(cjhopman): Should this be a diff against the node's deps?
    all_deps: ConfiguredTargetNodeDeps,
    platform_cfgs: BuckFixedSortedMap<TargetLabel, ConfigurationData>,
    // TODO(JakobDegen): Consider saving some memory by using a more tset like representation of
    // the plugin lists
    plugin_lists: PluginLists,
}

impl Debug for ConfiguredTargetNodeData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfiguredTargetNodeData")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl ConfiguredTargetNode {
    /// Creates a minimal ConfiguredTargetNode. Some operations may unexpectedly fail.
    pub fn testing_new(
        name: ConfiguredTargetLabel,
        rule_type: &str,
        execution_platform_resolution: ExecutionPlatformResolution,
        attrs: Vec<(&str, Attribute, OwnedCoercedAttr)>,
        call_stack: Option<StarlarkCallStack>,
    ) -> Self {
        use crate::nodes::unconfigured::testing::TargetNodeExt;

        let rule_type = RuleType::Starlark(Arc::new(StarlarkRuleType {
            path: BzlOrBxlPath::Bzl(ImportPath::testing_new("cell//pkg:rules.bzl")),
            name: rule_type.to_owned(),
        }));

        Self::new(
            name.dupe(),
            TargetNode::testing_new(name.unconfigured().dupe(), rule_type, attrs, call_stack),
            MatchedConfigurationSettingKeysWithCfg::new(
                ConfigurationNoExec::new(name.cfg().dupe()),
                MatchedConfigurationSettingKeys::empty(),
            ),
            BuckFixedSortedMap::new(),
            execution_platform_resolution,
            Vec::new(),
            Vec::new(),
            BuckFixedSortedMap::new(),
            PluginLists::new(),
        )
    }

    pub fn new(
        name: ConfiguredTargetLabel,
        target_node: TargetNode,
        resolved_configuration: MatchedConfigurationSettingKeysWithCfg,
        resolved_tr_configurations: BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>,
        execution_platform_resolution: ExecutionPlatformResolution,
        deps: Vec<ConfiguredTargetNode>,
        exec_deps: Vec<ConfiguredTargetNode>,
        platform_cfgs: BuckFixedSortedMap<TargetLabel, ConfigurationData>,
        plugin_lists: PluginLists,
    ) -> Self {
        Self(triomphe::Arc::new(Hashed::new(ConfiguredTargetNodeData {
            label: Hashed::new(name),
            target_node: TargetNodeOrForward::TargetNode(target_node),
            resolved_configuration,
            resolved_transition_configurations: resolved_tr_configurations,
            execution_platform_resolution,
            all_deps: ConfiguredTargetNodeDeps::new(deps, exec_deps),
            platform_cfgs,
            plugin_lists,
        })))
    }

    /// New `ConfiguredTargetNode` for a forward node for transitioned target.
    pub fn new_forward(
        // Forward node to create.
        name: ConfiguredTargetLabel,
        // The transitioned target node.
        transitioned_node: ConfiguredTargetNode,
    ) -> buck2_error::Result<Self> {
        assert_eq!(
            name.unconfigured(),
            transitioned_node.label().unconfigured(),
            "Forward node can only be created for the same unconfigured node"
        );
        assert_ne!(
            &name,
            transitioned_node.label(),
            "Creating a forward node to identical target label would be a mistake"
        );

        Ok(ConfiguredTargetNode(triomphe::Arc::new(Hashed::new(
            ConfiguredTargetNodeData {
                label: Hashed::new(name.dupe()),
                target_node: TargetNodeOrForward::Forward(transitioned_node.dupe()),
                // We have no attributes with selects, so resolved configurations is empty.
                resolved_configuration: MatchedConfigurationSettingKeysWithCfg::new(
                    name.cfg_pair().check_no_exec_cfg()?,
                    MatchedConfigurationSettingKeys::empty(),
                ),
                // We have no attributes to transition, so empty map is fine.
                resolved_transition_configurations: BuckFixedSortedMap::new(),
                // Set the execution platform equal to the transitioned node's execution platform
                // so we can call `buck2 test` on Forward.
                execution_platform_resolution: transitioned_node
                    .execution_platform_resolution()
                    .dupe(),
                plugin_lists: transitioned_node.plugin_lists().clone(),
                all_deps: ConfiguredTargetNodeDeps::new(vec![transitioned_node], vec![]),
                platform_cfgs: BuckFixedSortedMap::new(),
            },
        ))))
    }

    pub(crate) fn actual_attribute() -> &'static Attribute {
        static ATTRIBUTE: LazyLock<Attribute> = LazyLock::new(|| {
            Attribute::new(None, "", AttrType::configured_dep(ProviderIdSet::EMPTY))
                .expect("static Attribute::new failed")
        });
        &ATTRIBUTE
    }

    pub fn attr_as_target_compatible_with(
        attr: ConfiguredAttr,
    ) -> impl Iterator<Item = buck2_error::Result<ConfigurationSettingKey>> {
        let list = match attr.try_into_list() {
            Ok(list) => list,
            Err(e) => return Either::Left(iter::once(Err(e))),
        };
        Either::Right(list.into_iter().map(|val| {
            val.try_into_configuration_dep()
                .map(ConfigurationSettingKey)
        }))
    }

    pub fn execution_platform_resolution(&self) -> &ExecutionPlatformResolution {
        &self.0.execution_platform_resolution
    }

    /// Returns all deps for this node:
    /// - target ("normal") deps
    /// - execution deps
    /// - toolchain deps
    /// - configuration deps
    // TODO(cjhopman): Should this include configuration deps? Should it include the configuration deps that were inspected resolving selects?
    pub fn deps(&self) -> &[ConfiguredTargetNode] {
        &self.0.all_deps.all_deps
    }

    pub fn configuration_deps(&self) -> impl Iterator<Item = &ConfiguredTargetNode> {
        // Since we validate that all configuration dependencies are of kind Configuration,
        // we can use that to filter the deps.
        self.0
            .all_deps
            .deps()
            .iter()
            .filter(|x| x.rule_kind() == RuleKind::Configuration)
    }

    pub fn toolchain_deps(&self) -> impl Iterator<Item = &ConfiguredTargetNode> {
        // Since we validate that all toolchain dependencies are of kind Toolchain,
        // we can use that to filter the deps.
        self.0
            .all_deps
            .deps()
            .iter()
            .filter(|x| x.rule_kind() == RuleKind::Toolchain)
    }

    pub fn inputs(&self) -> impl Iterator<Item = CellPath> + '_ {
        self.as_ref().inputs()
    }

    #[inline]
    pub fn queries(
        &self,
    ) -> impl Iterator<Item = (String, ResolvedQueryLiterals<ConfiguredProvidersLabel>)> + '_ {
        self.as_ref().queries()
    }

    pub fn target_deps(&self) -> impl Iterator<Item = &ConfiguredTargetNode> {
        self.0
            .all_deps
            .deps()
            .iter()
            .filter(|x| x.rule_kind() == RuleKind::Normal)
    }

    pub fn exec_deps(&self) -> &[ConfiguredTargetNode] {
        self.0.all_deps.exec_deps()
    }

    /// Return the `tests` declared for this target configured in same target platform as this target.
    pub fn tests(&self) -> impl Iterator<Item = ConfiguredProvidersLabel> + use<> {
        #[derive(Default)]
        struct TestCollector {
            labels: Vec<ConfiguredProvidersLabel>,
        }

        impl ConfiguredAttrTraversal for TestCollector {
            fn dep(&mut self, _dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
                // ignored.
                Ok(())
            }

            fn label(&mut self, label: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
                self.labels.push(label.dupe());
                Ok(())
            }
        }

        let mut traversal = TestCollector::default();
        if let Some(tests) = self.get(TESTS_ATTRIBUTE.name, AttrInspectOptions::All) {
            tests.traverse(self.label().pkg(), &mut traversal).unwrap();
        }
        traversal.labels.into_iter()
    }

    pub fn label(&self) -> &ConfiguredTargetLabel {
        &self.0.label
    }

    #[inline]
    pub fn hashed_label(&self) -> Hashed<&ConfiguredTargetLabel> {
        self.0.label.as_ref()
    }

    pub fn target_node(&self) -> &TargetNode {
        match &self.0.target_node {
            TargetNodeOrForward::TargetNode(n) => n,
            TargetNodeOrForward::Forward(n) => n.target_node(),
        }
    }

    pub fn rule_type(&self) -> &RuleType {
        self.0.target_node.rule_type()
    }

    pub fn underlying_rule_type(&self) -> &RuleType {
        self.0.target_node.underlying_rule_type()
    }

    pub fn rule_kind(&self) -> RuleKind {
        self.0.target_node.rule_kind()
    }

    pub fn buildfile_path(&self) -> &BuildFilePath {
        self.0.target_node.buildfile_path()
    }

    pub fn is_visible_to(&self, target: &TargetLabel) -> buck2_error::Result<bool> {
        self.0.target_node.is_visible_to(target.dupe())
    }

    pub fn not_visible_to_error(
        &self,
        consumer: TargetLabel,
    ) -> crate::visibility::VisibilityError {
        self.0.target_node.not_visible_to_error(consumer)
    }

    #[inline]
    pub fn special_attr_or_none(&self, key: &str) -> Option<ConfiguredAttr> {
        self.as_ref().special_attr_or_none(key)
    }

    #[inline]
    pub fn special_attrs(&self) -> impl Iterator<Item = (&str, ConfiguredAttr)> {
        self.as_ref().special_attrs()
    }

    #[inline]
    pub fn oncall(&self) -> Option<&str> {
        self.as_ref().oncall()
    }

    pub fn attrs(
        &self,
        opts: AttrInspectOptions,
    ) -> impl ExactSizeIterator<Item = ConfiguredAttrFull<'_>> + '_ {
        self.as_ref().attrs(opts)
    }

    pub fn get<'a>(
        &'a self,
        attr: &str,
        opts: AttrInspectOptions,
    ) -> Option<ConfiguredAttrFull<'a>> {
        self.as_ref().get(attr, opts)
    }

    pub fn call_stack(&self) -> Option<String> {
        match &self.0.target_node {
            TargetNodeOrForward::TargetNode(n) => n.call_stack(),
            TargetNodeOrForward::Forward(n) => n.call_stack(),
        }
    }

    pub fn root_location(&self) -> Option<StarlarkTargetCallStackRoot> {
        match &self.0.target_node {
            TargetNodeOrForward::TargetNode(n) => n.root_location(),
            TargetNodeOrForward::Forward(n) => n.root_location(),
        }
    }

    /// Hash the fields that impact how this target is built.
    /// Don't do any recursive hashing of the dependencies.
    /// Hashes the attributes _after_ configuration, so changing unconfigured branches that
    /// are not taken will not change the hash.
    pub fn target_hash<H: Hasher>(&self, state: &mut H) {
        self.label().strong_hash(state);
        self.rule_type().strong_hash(state);
        self.attrs(AttrInspectOptions::All).for_each(|x| {
            // We deliberately don't hash the attribute, as if the value being passed to analysis
            // stays the same, we don't care if the attribute that generated it changed.
            x.name.strong_hash(state);
            x.value.strong_hash(state);
        });
    }

    /// Hash this target's unconfigured label, rule type, and configured attributes.
    ///
    /// Configurations are omitted from labels stored in attribute values. Consequently, changing
    /// configuration does not affect this hash unless it changes a configured attribute on this
    /// target. Dependency contents and other node metadata are not included.
    pub fn target_hash_without_configured_labels<H: Hasher>(&self, state: &mut H) {
        self.label().unconfigured().strong_hash(state);
        self.rule_type().strong_hash(state);

        let attrs = self.attrs(AttrInspectOptions::All);
        (attrs.len() as u64).strong_hash(state);
        attrs.for_each(|attr| {
            attr.name.strong_hash(state);
            attr.value.strong_hash_without_config(state);
        });
    }

    /// If this node is a forward node, return the target it forwards to.
    pub fn forward_target(&self) -> Option<&ConfiguredTargetNode> {
        match &self.0.target_node {
            TargetNodeOrForward::TargetNode(_) => None,
            TargetNodeOrForward::Forward(n) => Some(n),
        }
    }

    pub fn unwrap_forward(&self) -> &ConfiguredTargetNode {
        match self.forward_target() {
            None => self,
            Some(t) => t,
        }
    }

    #[inline]
    pub fn target_configuration(&self) -> &ConfigurationData {
        self.as_ref().0.get().label.cfg()
    }

    #[inline]
    pub fn execution_platform(&self) -> buck2_error::Result<&ExecutionPlatform> {
        self.as_ref().execution_platform_resolution().platform()
    }

    #[inline]
    pub fn uses_plugins(&self) -> &[PluginKind] {
        self.as_ref().uses_plugins()
    }

    pub fn plugin_lists(&self) -> &PluginLists {
        &self.0.plugin_lists
    }

    #[inline]
    pub fn as_ref(&self) -> ConfiguredTargetNodeRef<'_> {
        ConfiguredTargetNodeRef(triomphe::Arc::borrow_arc(&self.0))
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        triomphe::Arc::ptr_eq(&self.0, &other.0)
    }
}

/// The representation of the deps for a ConfiguredTargetNode. Provides the operations we require
/// (iteration, eq, and hash), but guarantees those aren't recursive of the dep nodes' data.
#[derive(Allocative, Pagable)]
struct ConfiguredTargetNodeDeps {
    /// Number of deps, excluding exec deps. Used as an index to retrieve exec_deps
    deps_count: usize,
    /// (target deps and toolchain deps) followed by `exec_deps`.
    all_deps: MiniBoxSlice<ConfiguredTargetNode>,
}

impl ConfiguredTargetNodeDeps {
    fn new(deps: Vec<ConfiguredTargetNode>, exec_deps: Vec<ConfiguredTargetNode>) -> Self {
        if deps.is_empty() {
            ConfiguredTargetNodeDeps {
                deps_count: 0,
                all_deps: exec_deps.into(),
            }
        } else if exec_deps.is_empty() {
            ConfiguredTargetNodeDeps {
                deps_count: deps.len(),
                all_deps: deps.into(),
            }
        } else {
            ConfiguredTargetNodeDeps {
                deps_count: deps.len(),
                all_deps: deps.into_iter().chain(exec_deps).collect(),
            }
        }
    }

    fn deps(&self) -> &[ConfiguredTargetNode] {
        &self.all_deps[..self.deps_count]
    }

    fn exec_deps(&self) -> &[ConfiguredTargetNode] {
        &self.all_deps[self.deps_count..]
    }
}

/// We only do Arc comparisons here. The rationale is that each ConfiguredTargetNode should only
/// ever exist in one instance in the graph, so if the ptr eq doesn't match, then we don't do a
/// deep comparison.
impl PartialEq for ConfiguredTargetNodeDeps {
    fn eq(&self, other: &Self) -> bool {
        let ConfiguredTargetNodeDeps {
            deps_count,
            all_deps,
        } = self;
        *deps_count == other.deps_count && all_deps.len() == other.all_deps.len() && {
            let it1 = all_deps.iter();
            let it2 = other.all_deps.iter();
            it1.zip(it2).all(|(x, y)| triomphe::Arc::ptr_eq(&x.0, &y.0))
        }
    }
}

impl Eq for ConfiguredTargetNodeDeps {}

/// This has historically only hashed the labels. This may or may not be right, because it means
/// two nodes that reference different instances of a given dependency will hash equal, even if
/// the dependency has definitely changed (e.g. because its own deps changed).
impl Hash for ConfiguredTargetNodeDeps {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let ConfiguredTargetNodeDeps {
            deps_count,
            all_deps,
        } = self;
        state.write_usize(*deps_count);
        for node in &**all_deps {
            node.label().hash(state);
        }
    }
}

/// Like `&ConfiguredTargetNode`, but cheaper (one fewer indirection).
#[derive(Debug, Copy, Clone)]
pub struct ConfiguredTargetNodeRef<'a>(triomphe::ArcBorrow<'a, Hashed<ConfiguredTargetNodeData>>);

impl Dupe for ConfiguredTargetNodeRef<'_> {}

impl<'a> ConfiguredTargetNodeRef<'a> {
    #[inline]
    pub fn to_owned(self) -> ConfiguredTargetNode {
        ConfiguredTargetNode(triomphe::ArcBorrow::clone_arc(&self.0))
    }

    #[inline]
    pub fn deps(self) -> &'a [ConfiguredTargetNode] {
        &self.0.get().all_deps.all_deps
    }

    #[inline]
    pub fn ptr_eq(self, other: Self) -> bool {
        triomphe::ArcBorrow::ptr_eq(&self.0, &other.0)
    }

    #[inline]
    pub fn label(self) -> &'a ConfiguredTargetLabel {
        self.0.get().label.key()
    }

    #[inline]
    pub fn hashed_label(self) -> Hashed<&'a ConfiguredTargetLabel> {
        self.0.get().label.as_ref()
    }

    fn attr_configuration_context(self) -> AttrConfigurationContextImpl<'a> {
        AttrConfigurationContextImpl::new(
            self.label().dupe(),
            &self.0.get().resolved_configuration,
            &self.0.get().execution_platform_resolution,
            &self.0.get().resolved_transition_configurations,
            &self.0.get().platform_cfgs,
            self.0.get().target_node.unmatched_select_incompatible(),
        )
    }

    pub fn oncall(self) -> Option<&'a str> {
        self.0.get().target_node.oncall()
    }

    pub fn special_attr_or_none(&self, key: &str) -> Option<ConfiguredAttr> {
        match key {
            TYPE => Some(ConfiguredAttr::String(StringLiteral(
                self.rule_type().name().into(),
            ))),
            DEPS => Some(ConfiguredAttr::List(
                self.deps()
                    .iter()
                    .map(|t| {
                        ConfiguredAttr::Label(ConfiguredProvidersLabel::new(
                            t.label().dupe(),
                            ProvidersName::Default,
                        ))
                    })
                    .collect(),
            )),
            PACKAGE => Some(ConfiguredAttr::String(StringLiteral(ArcStr::from(
                self.buildfile_path().to_string(),
            )))),
            ONCALL => Some(match self.oncall() {
                None => ConfiguredAttr::None,
                Some(x) => ConfiguredAttr::String(StringLiteral(ArcStr::from(x))),
            }),
            TARGET_CONFIGURATION => Some(ConfiguredAttr::String(StringLiteral(ArcStr::from(
                self.0.label.cfg().to_string(),
            )))),
            EXECUTION_PLATFORM => Some(ConfiguredAttr::String(StringLiteral(
                self.0
                    .execution_platform_resolution
                    .platform()
                    .map_or_else(|_| ArcStr::from("<NONE>"), |v| ArcStr::from(v.id())),
            ))),
            EXECUTION_GROUP_PLATFORMS => self.execution_group_platforms_as_attr(),
            PLUGINS => Some(self.plugins_as_attr()),
            _ => None,
        }
    }

    fn execution_group_platforms_as_attr(self) -> Option<ConfiguredAttr> {
        let resolution = &self.0.execution_platform_resolution;
        if !resolution.has_execution_groups() {
            return None;
        }
        Some(ConfiguredAttr::Dict(
            resolution
                .execution_group_platforms()
                .map(|(group, platform, _)| {
                    (
                        ConfiguredAttr::String(StringLiteral(ArcStr::from(group.name()))),
                        ConfiguredAttr::String(StringLiteral(
                            platform
                                .map_or_else(|| ArcStr::from("<NONE>"), |p| ArcStr::from(p.id())),
                        )),
                    )
                })
                .collect(),
        ))
    }

    pub fn special_attrs(self) -> impl Iterator<Item = (&'a str, ConfiguredAttr)> {
        [
            TYPE,
            DEPS,
            PACKAGE,
            ONCALL,
            TARGET_CONFIGURATION,
            EXECUTION_PLATFORM,
            PLUGINS,
        ]
        .into_iter()
        .map(move |key| {
            (
                key,
                // use unwrap here, if fail here it means we iter over a key not in the match list from `special_attr_or_none`
                self.special_attr_or_none(key).unwrap(),
            )
        })
        .chain(
            self.execution_group_platforms_as_attr()
                .map(|v| (EXECUTION_GROUP_PLATFORMS, v)),
        )
    }

    /// Not implemented via [`Self::filter_attrs`]: the trivial filter would
    /// reset the iterator's `size_hint` lower bound to 0, and callers rely on
    /// the exact size to pre-allocate.
    pub fn attrs(
        self,
        opts: AttrInspectOptions,
    ) -> impl ExactSizeIterator<Item = ConfiguredAttrFull<'a>> + 'a {
        match &self.0.get().target_node {
            TargetNodeOrForward::TargetNode(n) => Either::Left(n.attrs(opts).map(move |a| {
                a.configure(&self.attr_configuration_context())
                    .expect_compatible("checked attr configuration in constructor")
            })),
            TargetNodeOrForward::Forward(n) => Either::Right(iter::once(actual_attr(n))),
        }
    }

    /// Like [`attrs`](Self::attrs), but only configures attributes whose name
    /// and definition pass `filter`. Configuration is the expensive step, so
    /// callers that need a subset should filter with this rather than
    /// discarding configured attrs.
    pub fn filter_attrs(
        self,
        opts: AttrInspectOptions,
        filter: impl Fn(&str, &Attribute) -> bool + 'a,
    ) -> impl Iterator<Item = ConfiguredAttrFull<'a>> + 'a {
        match &self.0.get().target_node {
            TargetNodeOrForward::TargetNode(n) => Either::Left(
                n.attrs(opts)
                    .filter(move |a| filter(a.name, a.attr))
                    .map(move |a| {
                        a.configure(&self.attr_configuration_context())
                            .expect_compatible("checked attr configuration in constructor")
                    }),
            ),
            TargetNodeOrForward::Forward(n) => Either::Right(
                filter(ACTUAL_ATTR_NAME, ConfiguredTargetNode::actual_attribute())
                    .then(|| actual_attr(n))
                    .into_iter(),
            ),
        }
    }

    pub fn get(self, attr: &str, opts: AttrInspectOptions) -> Option<ConfiguredAttrFull<'a>> {
        match &self.0.get().target_node {
            TargetNodeOrForward::TargetNode(n) => n.attr_or_none(attr, opts).map(|v| {
                v.configure(&self.attr_configuration_context())
                    .expect_compatible("checked attr configuration in constructor")
            }),
            TargetNodeOrForward::Forward(n) => (attr == ACTUAL_ATTR_NAME).then(|| actual_attr(n)),
        }
    }

    pub(crate) fn attr_matches(
        self,
        key: &str,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        let coerced = match &self.0.get().target_node {
            TargetNodeOrForward::TargetNode(n) => n.attr_or_none(key, AttrInspectOptions::All),
            TargetNodeOrForward::Forward(n) if key == ACTUAL_ATTR_NAME => {
                return actual_attr(n).value.any_matches(filter);
            }
            TargetNodeOrForward::Forward(_) => None,
        };
        match coerced {
            Some(attr) => match attr.value.kind() {
                CoercedAttrRefKind::String(v) | CoercedAttrRefKind::EnumVariant(v) => filter(v),
                CoercedAttrRefKind::List(values)
                    if values.iter().all(|v| {
                        matches!(
                            v.kind(),
                            CoercedAttrRefKind::String(_) | CoercedAttrRefKind::EnumVariant(_)
                        )
                    }) =>
                {
                    attr.value.any_matches(filter)
                }
                _ => attr
                    .configure(&self.attr_configuration_context())
                    .expect_compatible("checked attr configuration in constructor")
                    .value
                    .any_matches(filter),
            },
            None => match self.special_attr_or_none(key) {
                Some(attr) => attr.any_matches(filter),
                None => Ok(false),
            },
        }
    }

    pub fn inputs(self) -> impl Iterator<Item = CellPath> + 'a {
        struct InputsCollector {
            inputs: Vec<CellPath>,
        }
        impl ConfiguredAttrTraversal for InputsCollector {
            fn dep(&mut self, _dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
                Ok(())
            }

            fn input(&mut self, path: SourcePathRef) -> buck2_error::Result<()> {
                self.inputs.push(path.to_cell_path());
                Ok(())
            }
        }
        let mut traversal = InputsCollector { inputs: Vec::new() };
        for a in self.attrs(AttrInspectOptions::All) {
            a.traverse(self.label().pkg(), &mut traversal)
                .expect("inputs collector shouldn't return errors");
        }
        traversal.inputs.into_iter()
    }

    // TODO(cjhopman): switch to for_each_query?
    /// Returns distinct queries and their resolved literals in first-occurrence order.
    pub fn queries(
        self,
    ) -> impl Iterator<Item = (String, ResolvedQueryLiterals<ConfiguredProvidersLabel>)> + 'a {
        struct Traversal {
            queries: BuckMutInsertionSet<(String, ResolvedQueryLiterals<ConfiguredProvidersLabel>)>,
        }
        let mut traversal = Traversal {
            queries: BuckMutInsertionSet::default(),
        };
        impl ConfiguredAttrTraversal for Traversal {
            fn dep(&mut self, _dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
                // ignored.
                Ok(())
            }

            fn query(
                &mut self,
                query: &str,
                resolved_literals: &ResolvedQueryLiterals<ConfiguredProvidersLabel>,
            ) -> buck2_error::Result<()> {
                self.queries
                    .insert((query.to_owned(), resolved_literals.clone()));
                Ok(())
            }
        }

        for a in self.filter_attrs(AttrInspectOptions::All, |_, attr| {
            attr.coercer().0.may_have_queries
        }) {
            a.traverse(self.label().pkg(), &mut traversal).unwrap();
        }
        traversal.queries.into_iter()
    }

    pub fn rule_type(self) -> &'a RuleType {
        self.0.get().target_node.rule_type()
    }

    pub fn execution_platform_resolution(self) -> &'a ExecutionPlatformResolution {
        &self.0.get().execution_platform_resolution
    }

    pub fn uses_plugins(self) -> &'a [PluginKind] {
        match &self.0.get().target_node {
            TargetNodeOrForward::TargetNode(target_node) => target_node.uses_plugins(),
            TargetNodeOrForward::Forward(_) => &[],
        }
    }

    fn plugins_as_attr(self) -> ConfiguredAttr {
        let mut kinds = Vec::new();
        for (kind, plugins) in self.plugin_lists().iter_by_kind() {
            // Using plugin dep here is a bit of an abuse. However, there's no
            // `ConfiguredAttr::TargetLabel` type, and it also seems excessive to add one for this
            // reason alone
            let plugins = plugins
                .map(|(target, _)| ConfiguredAttr::PluginDep(target.dupe(), kind.dupe()))
                .collect();
            kinds.push((
                ConfiguredAttr::String(StringLiteral(ArcStr::from(kind.as_str()))),
                ConfiguredAttr::List(plugins),
            ));
        }
        ConfiguredAttr::Dict(kinds.into_iter().collect())
    }

    pub fn plugin_lists(self) -> &'a PluginLists {
        &self.0.get().plugin_lists
    }

    pub fn buildfile_path(self) -> &'a BuildFilePath {
        self.0.get().target_node.buildfile_path()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::hash_map::DefaultHasher;

    use buck2_core::configuration::config_setting::ConfigSettingData;
    use buck2_core::configuration::constraints::ConstraintKey;
    use buck2_core::configuration::constraints::ConstraintValue;
    use buck2_core::configuration::data::ConfigurationDataData;
    use buck2_core::configuration::data::ExecPlatformMarking;

    use super::*;
    use crate::attrs::attr_type::arg::MacroBase;
    use crate::attrs::attr_type::arg::QueryExpansion;
    use crate::attrs::attr_type::arg::StringWithMacros;
    use crate::attrs::attr_type::arg::StringWithMacrosPart;
    use crate::attrs::attr_type::query::QueryAttrBase;
    use crate::attrs::attr_type::query::QueryMacroBase;
    use crate::configuration::resolved::ConfigurationNode;
    use crate::nodes::unconfigured::testing::TargetNodeExt;

    /// `literal` is the target's spelling inside `text`; `target` its
    /// resolved label.
    fn query(text: &str, literal: &str, target: &str) -> QueryAttrBase<ProvidersLabel> {
        let start = text.find(literal).unwrap();
        QueryAttrBase {
            query: text.to_owned(),
            resolved_literals: ResolvedQueryLiterals(BTreeMap::from([(
                (start, literal.len()),
                ProvidersLabel::default_for(TargetLabel::testing_parse(target)),
            )])),
        }
    }

    /// Asserts `actual` is `expected` as reported by `queries()`: the
    /// canonical reprint, with one literal whose span slices the reprint to
    /// the configured label's unconfigured spelling.
    fn assert_query(
        actual: &(String, ResolvedQueryLiterals<ConfiguredProvidersLabel>),
        expected_reprint: &str,
        expected: &QueryAttrBase<ProvidersLabel>,
        cfg: &ConfigurationData,
    ) {
        assert_eq!(actual.0, expected_reprint);
        let [label]: [&ProvidersLabel; 1] = expected
            .resolved_literals
            .0
            .values()
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let ((off, len), configured) = actual.1.0.iter().next().unwrap();
        assert_eq!(actual.1.0.len(), 1);
        assert_eq!(configured, &label.configure(cfg.dupe()));
        assert_eq!(&actual.0[*off..*off + *len], label.to_string());
    }

    fn node(
        cfg: ConfigurationData,
        attrs: Vec<(&str, Attribute, OwnedCoercedAttr)>,
    ) -> ConfiguredTargetNode {
        ConfiguredTargetNode::testing_new(
            TargetLabel::testing_parse("root//pkg:owner").configure(cfg),
            "test_rule",
            ExecutionPlatformResolution::new_for_testing(None, Vec::new()),
            attrs,
            None,
        )
    }

    /// The attribute spells its literal relative and the macros spell it
    /// in full: all four are one query.
    #[test]
    fn test_queries_deduplicate_attributes_and_macro_expansions() {
        let attr_query = query("deps(:dep)", ":dep", "root//pkg:dep");
        let macro_query = query("deps(root//pkg:dep)", "root//pkg:dep", "root//pkg:dep");
        let query_attr = OwnedCoercedAttr::new_with(|b| b.encode_query(&attr_query)).unwrap();
        let macros = [
            QueryExpansion::Target,
            QueryExpansion::Output,
            QueryExpansion::TargetAndOutput(None),
        ]
        .into_iter()
        .map(|expansion_type| {
            StringWithMacrosPart::Macro(
                true,
                MacroBase::Query(Box::new(QueryMacroBase {
                    expansion_type,
                    query: macro_query.clone(),
                })),
            )
        })
        .collect();
        let attrs = vec![
            (
                "first",
                Attribute::new_const(None, "", AttrType::query()),
                query_attr.clone(),
            ),
            (
                "second",
                Attribute::new_const(None, "", AttrType::query()),
                query_attr,
            ),
            (
                "command",
                Attribute::new_const(None, "", AttrType::arg(false)),
                OwnedCoercedAttr::new_with(|b| b.encode_arg(&StringWithMacros::ManyParts(macros)))
                    .unwrap(),
            ),
        ];

        for cfg in [
            ConfigurationData::testing_new(),
            ConfigurationData::unspecified(),
        ] {
            let node = node(cfg.dupe(), attrs.clone());
            let queries: Vec<_> = node.queries().collect();
            assert_eq!(queries.len(), 1);
            assert_query(&queries[0], "deps(root//pkg:dep)", &attr_query, &cfg);
        }
    }

    #[test]
    fn test_queries_preserve_order_and_distinct_literal_bindings() {
        let first = query("deps(:dep, 1)", ":dep", "root//pkg:dep");
        let second = query("deps(:dep)", ":dep", "root//pkg:dep");
        let third = query(
            "deps(root//other:dep)",
            "root//other:dep",
            "root//other:dep",
        );
        let macros = [&first, &second, &first, &third, &second]
            .into_iter()
            .map(|query| {
                StringWithMacrosPart::Macro(
                    false,
                    MacroBase::Query(Box::new(QueryMacroBase {
                        expansion_type: QueryExpansion::Target,
                        query: query.clone(),
                    })),
                )
            })
            .collect();
        let cfg = ConfigurationData::testing_new();
        let node = node(
            cfg.dupe(),
            vec![(
                "command",
                Attribute::new_const(None, "", AttrType::arg(false)),
                OwnedCoercedAttr::new_with(|b| b.encode_arg(&StringWithMacros::ManyParts(macros)))
                    .unwrap(),
            )],
        );
        let queries: Vec<_> = node.queries().collect();
        assert_eq!(queries.len(), 3);
        for (actual, (reprint, expected)) in queries.iter().zip([
            ("deps(root//pkg:dep, 1)", first),
            ("deps(root//pkg:dep)", second),
            ("deps(root//other:dep)", third),
        ]) {
            assert_query(actual, reprint, &expected, &cfg);
        }
    }

    fn configured_node(label: ConfiguredTargetLabel, value: &str) -> ConfiguredTargetNode {
        configured_node_with_rule_type(label, "test_rule", value)
    }

    fn configured_node_with_rule_type(
        label: ConfiguredTargetLabel,
        rule_type: &str,
        value: &str,
    ) -> ConfiguredTargetNode {
        ConfiguredTargetNode::testing_new(
            label,
            rule_type,
            ExecutionPlatformResolution::new_for_testing(None, Vec::new()),
            vec![(
                "value",
                Attribute::new_const(None, "", AttrType::string()),
                OwnedCoercedAttr::new_with(|b| b.encode_string(value)).unwrap(),
            )],
            None,
        )
    }

    /// A node in `cfg` with one dep attribute, which configures to `dep`
    /// in `cfg`.
    fn configured_node_with_dep(dep: TargetLabel, cfg: ConfigurationData) -> ConfiguredTargetNode {
        ConfiguredTargetNode::testing_new(
            TargetLabel::testing_parse("cell//pkg:root").configure(cfg),
            "test_rule",
            ExecutionPlatformResolution::new_for_testing(None, Vec::new()),
            vec![(
                "dep",
                Attribute::new_const(
                    None,
                    "",
                    AttrType::dep(ProviderIdSet::EMPTY, PluginKindSet::EMPTY),
                ),
                OwnedCoercedAttr::new_with(|b| b.encode_dep(&ProvidersLabel::default_for(dep)))
                    .unwrap(),
            )],
            None,
        )
    }

    fn target_hash_without_configured_labels(node: &ConfiguredTargetNode) -> u64 {
        let mut hasher = DefaultHasher::new();
        node.target_hash_without_configured_labels(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn target_hash_without_configured_labels_ignores_configuration() {
        let label = TargetLabel::testing_parse("cell//pkg:target");
        let first = configured_node(label.configure(ConfigurationData::testing_new()), "value");
        let second = configured_node(label.configure(ConfigurationData::unspecified()), "value");
        let changed = configured_node(label.configure(ConfigurationData::unspecified()), "changed");

        assert_eq!(
            target_hash_without_configured_labels(&first),
            target_hash_without_configured_labels(&second),
        );
        assert_ne!(
            target_hash_without_configured_labels(&first),
            target_hash_without_configured_labels(&changed),
        );
    }

    #[test]
    fn target_hash_without_configured_labels_tracks_selected_value() {
        let label = TargetLabel::testing_parse("cell//pkg:target");
        let branches = ["cell//cfg:linux", "cell//cfg:macos"];
        let target_node = TargetNode::testing_new(
            label,
            RuleType::Starlark(Arc::new(StarlarkRuleType {
                path: BzlOrBxlPath::Bzl(ImportPath::testing_new("cell//pkg:rules.bzl")),
                name: "test_rule".to_owned(),
            })),
            vec![(
                "value",
                Attribute::new(None, "", AttrType::string()).expect("valid string attribute"),
                OwnedCoercedAttr::new_with(|b| {
                    let same = b.encode_string("same")?;
                    let changed = b.encode_string("changed")?;
                    let entries: Vec<_> = branches
                        .iter()
                        .map(|key| (ConfigurationSettingKey::testing_parse(key), same))
                        .collect();
                    b.encode_selector(&entries, Some(changed))
                })
                .expect("valid select"),
            )],
            None,
        );

        let configure = |platform: &str| {
            let constraints = BTreeMap::from_iter([(
                ConstraintKey::testing_new("cell//cfg:os"),
                ConstraintValue::testing_new(platform, None),
            )]);
            let cfg = ConfigurationData::from_platform(
                "cell//cfg:platform".to_owned(),
                ConfigurationDataData::new(constraints.clone()),
                ExecPlatformMarking::Unmarked,
            )
            .expect("valid platform configuration");
            let matched_keys = MatchedConfigurationSettingKeys::new(
                branches
                    .iter()
                    .map(|key| {
                        (
                            ConfigurationSettingKey::testing_parse(key),
                            ConfigurationNode::new(
                                (*key == platform)
                                    .then(|| ConfigSettingData::testing_new(constraints.clone())),
                            ),
                        )
                    })
                    .collect(),
            );
            ConfiguredTargetNode::new(
                label.configure(cfg.dupe()),
                target_node.dupe(),
                MatchedConfigurationSettingKeysWithCfg::new(
                    ConfigurationNoExec::new(cfg),
                    matched_keys,
                ),
                BuckFixedSortedMap::new(),
                ExecutionPlatformResolution::new_for_testing(None, Vec::new()),
                Vec::new(),
                Vec::new(),
                BuckFixedSortedMap::new(),
                PluginLists::new(),
            )
        };

        let linux = configure("cell//cfg:linux");
        let macos = configure("cell//cfg:macos");
        let windows = configure("cell//cfg:windows");

        for (node, expected) in [(&linux, "same"), (&macos, "same"), (&windows, "changed")] {
            assert_eq!(
                node.get("value", AttrInspectOptions::All)
                    .expect("value attribute exists")
                    .value,
                ConfiguredAttr::String(StringLiteral(expected.into())),
            );
        }
        assert_ne!(linux.label().cfg(), macos.label().cfg());
        assert_eq!(
            target_hash_without_configured_labels(&linux),
            target_hash_without_configured_labels(&macos),
            "different select branches with identical values must hash equally",
        );
        assert_ne!(
            target_hash_without_configured_labels(&linux),
            target_hash_without_configured_labels(&windows),
            "selecting a different value must change the hash",
        );
    }

    #[test]
    fn target_hash_without_configured_labels_tracks_rule_type() {
        let label = TargetLabel::testing_parse("cell//pkg:target");
        let cfg = ConfigurationData::testing_new();
        let configured_label = label.configure(cfg);
        let first = configured_node_with_rule_type(configured_label.dupe(), "first_rule", "value");
        let second = configured_node_with_rule_type(configured_label, "second_rule", "value");

        assert_ne!(
            target_hash_without_configured_labels(&first),
            target_hash_without_configured_labels(&second),
        );
    }

    #[test]
    fn target_hash_without_configured_labels_tracks_target_label() {
        let cfg = ConfigurationData::testing_new();
        let first = configured_node(
            TargetLabel::testing_parse("cell//pkg:first").configure(cfg.clone()),
            "value",
        );
        let second = configured_node(
            TargetLabel::testing_parse("cell//pkg:second").configure(cfg),
            "value",
        );

        assert_ne!(
            target_hash_without_configured_labels(&first),
            target_hash_without_configured_labels(&second),
            "changing the unconfigured target label must change the hash",
        );
    }

    #[test]
    fn target_hash_without_configured_labels_hashes_only_unconfigured_dependency_label() {
        let dep = TargetLabel::testing_parse("cell//dep:target");
        let changed_dep = TargetLabel::testing_parse("cell//dep:changed");

        let first = configured_node_with_dep(dep.dupe(), ConfigurationData::testing_new());
        let second = configured_node_with_dep(dep, ConfigurationData::unspecified());
        let changed = configured_node_with_dep(changed_dep, ConfigurationData::unspecified());
        assert_ne!(
            first.get("dep", AttrInspectOptions::All).unwrap().value,
            second.get("dep", AttrInspectOptions::All).unwrap().value,
            "the configured dep labels should differ in configuration",
        );

        assert_eq!(
            target_hash_without_configured_labels(&first),
            target_hash_without_configured_labels(&second),
        );
        assert_ne!(
            target_hash_without_configured_labels(&second),
            target_hash_without_configured_labels(&changed),
        );
    }

    #[test]
    fn target_hash_without_configured_labels_does_not_hash_dependencies_recursively() {
        let label = TargetLabel::testing_parse("cell//pkg:target");
        let first_cfg = ConfigurationData::testing_new();
        let second_cfg = ConfigurationData::unspecified();

        let first_dep = configured_node(label.configure(first_cfg.clone()), "value");
        let first =
            ConfiguredTargetNode::new_forward(label.configure(second_cfg.clone()), first_dep)
                .expect("forward target should use a different configuration");
        let second_dep = configured_node(label.configure(second_cfg.clone()), "value");
        let second =
            ConfiguredTargetNode::new_forward(label.configure(first_cfg.clone()), second_dep)
                .expect("forward target should use a different configuration");
        let changed_dep = configured_node(label.configure(second_cfg), "changed");
        let changed = ConfiguredTargetNode::new_forward(label.configure(first_cfg), changed_dep)
            .expect("forward target should use a different configuration");

        assert_eq!(
            target_hash_without_configured_labels(&first),
            target_hash_without_configured_labels(&second),
        );
        assert_eq!(
            target_hash_without_configured_labels(&second),
            target_hash_without_configured_labels(&changed),
        );
    }
}
