/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use core::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::OnceLock;

use allocative::Allocative;
use buck2_core::build_file_path::BuildFilePath;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::package::source_path::SourcePathRef;
use buck2_core::plugins::PluginKind;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::internal_error;
use buck2_util::arc_str::ArcStr;
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

use crate::attrs::arena::CoercedAttrArena;
use crate::attrs::arena::OwnedCoercedAttr;
use crate::attrs::attr_type::configuration_dep::ConfigurationDepKind;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced_attr_full::CoercedAttrFull;
use crate::attrs::coerced_deps_collector::CoercedDeps;
use crate::attrs::coerced_deps_collector::CoercedDepsCollector;
use crate::attrs::display::AttrDisplayWithContextExt;
use crate::attrs::inspect_options::AttrInspectOptions;
use crate::attrs::spec::AttributeId;
use crate::attrs::spec::AttributeSpec;
use crate::attrs::spec::internal::DEFAULT_TARGET_PLATFORM_ATTRIBUTE;
use crate::attrs::spec::internal::METADATA_ATTRIBUTE;
use crate::attrs::spec::internal::TARGET_MODIFIERS_ATTRIBUTE;
use crate::attrs::spec::internal::TESTS_ATTRIBUTE;
use crate::attrs::spec::internal::VISIBILITY_ATTRIBUTE;
use crate::attrs::special_attr::SpecialAttr;
use crate::attrs::traversal::CoercedAttrTraversal;
use crate::attrs::values::AttrValues;
use crate::attrs::values::UnsealedAttrValues;
use crate::call_stack::StarlarkCallStack;
use crate::call_stack::StarlarkTargetCallStackRoot;
use crate::metadata::map::MetadataMap;
use crate::modifiers::CfgModifiers;
use crate::nodes::attributes::CONFIGURATION_DEPS;
use crate::nodes::attributes::DEPS;
use crate::nodes::attributes::ONCALL;
use crate::nodes::attributes::PACKAGE;
use crate::nodes::attributes::PACKAGE_CFG_MODIFIERS;
use crate::nodes::attributes::TYPE;
use crate::package::Package;
use crate::rule::Rule;
use crate::rule::RuleExecutionGroup;
use crate::rule_type::RuleType;
use crate::visibility::VisibilityError;
use crate::visibility::VisibilityIntersection;
use crate::visibility::VisibilitySpecification;

/// Describes a target including its name, type, and the values that the user provided.
/// Some information (e.g. deps) is extracted eagerly, most is in the attrs map and needs to be
/// accessed via attribute visitors.
///
/// For attributes, to avoid duplicating data across many nodes the TargetNode itself doesn't store
/// the attribute names and it doesn't store an entry for something that has a default value. All
/// that information is contained in the AttributeSpec. This means that to access an attribute we
/// need to look at both the attrs held by the TargetNode and the information in the AttributeSpec.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Allocative, Pagable)]
pub struct TargetNode(triomphe::Arc<TargetNodeData>);

// SAFETY: as for `ConfiguredTargetNode`.
unsafe impl Send for TargetNode {}
unsafe impl Sync for TargetNode {}

fn _assert_send_sync(TargetNode(inner): &TargetNode) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(inner);
}

impl Dupe for TargetNode {}

impl Deref for TargetNode {
    type Target = TargetNodeData;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The kind of the rule, denoting where it can be used and how.
#[derive(
    Debug, Copy, Clone, Dupe, Eq, PartialEq, Hash, StrongHash, Pagable, Allocative
)]
pub enum RuleKind {
    /// A normal rule with no special properties.
    Normal,
    /// A configuration rule, meaning it is usable in a configuration context.
    Configuration,
    /// A toolchain rule, meaning it is only usable as a toolchain dep.
    Toolchain,
}

impl RuleKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RuleKind::Normal => "normal",
            RuleKind::Configuration => "configuration",
            RuleKind::Toolchain => "toolchain",
        }
    }
}

impl fmt::Display for RuleKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub struct TargetNodeData {
    /// Rule type for this target.
    pub rule: Arc<Rule>,

    /// Package.
    package: Arc<Package>,

    label: TargetLabel,

    /// The attribute->value mapping for this rule. It's guaranteed that if an attribute does not
    /// have a value here, it does have a default value in the AttributeSpec.
    attributes: AttrValues,

    /// Cache of the deps derived from the attributes, computed lazily on first access. Not
    /// populating it at package load time saves memory for the (typically many) nodes in loaded
    /// packages whose deps are never requested.
    #[pagable(discard = "Default::default()")]
    deps_cache: LazyCoercedDeps,

    /// Call stack for the target.
    #[pagable(discard = "None")]
    call_stack: Option<StarlarkCallStack>,

    /// Config modifiers set in the package this target belongs to
    package_cfg_modifiers: Option<CfgModifiers>,

    test_config_unification_rollout: bool,
}

/// Lazily computed [`CoercedDeps`] for a target node.
///
/// This is a pure function of the target's attributes, so it is ignored for equality and hashing
/// (the attributes are compared/hashed instead) and is recomputed on demand after being discarded
/// when the node is paged out.
#[derive(Debug, Default, Allocative)]
struct LazyCoercedDeps(OnceLock<CoercedDeps>);

impl PartialEq for LazyCoercedDeps {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for LazyCoercedDeps {}

impl Hash for LazyCoercedDeps {
    fn hash<H: Hasher>(&self, _state: &mut H) {}
}

impl TargetNodeData {
    pub fn is_toolchain_rule(&self) -> bool {
        self.rule.rule_kind == RuleKind::Toolchain
    }

    pub fn rule_type(&self) -> &RuleType {
        &self.rule.rule_type
    }

    pub fn oncall(&self) -> Option<&str> {
        self.package.oncall.as_ref().map(|x| x.as_str())
    }

    pub fn call_stack(&self) -> Option<String> {
        self.call_stack.as_ref().map(|s| s.to_string())
    }

    pub fn root_location(&self) -> Option<StarlarkTargetCallStackRoot> {
        self.call_stack.as_ref().and_then(|s| s.root_location())
    }

    pub fn package_cfg_modifiers(&self) -> Option<&CfgModifiers> {
        self.package_cfg_modifiers.as_ref()
    }

    pub fn test_config_unification_rollout(&self) -> bool {
        self.test_config_unification_rollout
    }

    fn deps_cache(&self) -> &CoercedDeps {
        self.deps_cache.0.get_or_init(|| {
            let mut collector = CoercedDepsCollector::new();
            for a in self
                .rule
                .attributes
                .attrs(&self.attributes, AttrInspectOptions::All)
            {
                // Infallible, which is what lets the deps accessors stay non-`Result`. Every
                // `CoercedDepsCollector` callback returns `Ok`, and the only errors `traverse`
                // raises on its own are internal invariant checks: a value whose kind does not
                // fit its attribute type (the coercer established that pairing) and a missing
                // package (we pass `Some`). Either would be a buck2 bug, and panicking is the
                // only option here because the callers cannot surface an error. `OnceLock`
                // does not poison, so the cell stays empty and a later access retries.
                a.traverse(self.label.pkg(), &mut collector)
                    .expect("should be unreachable: collector callbacks all return Ok");
            }
            CoercedDeps::from(collector)
        })
    }

    /// Intersection inherited from `enforce_visibility_intersection()`.
    /// Empty = no restriction.
    /// Stored on `Package` (per build file), so all targets in the same BUCK
    /// file share the same allocation.
    pub fn visibility_intersection(&self) -> &VisibilityIntersection {
        &self.package.visibility_intersection
    }
}

/// A target as recorded while its package is still evaluating: everything a
/// [`TargetNode`] holds except the package's attr arena, which exists only
/// once evaluation ends.
#[derive(Debug)]
pub struct UnsealedTargetNode {
    pub rule: Arc<Rule>,
    pub package: Arc<Package>,
    pub label: TargetLabel,
    pub attributes: UnsealedAttrValues,
    pub call_stack: Option<StarlarkCallStack>,
    pub package_cfg_modifiers: Option<CfgModifiers>,
    pub test_config_unification_rollout: bool,
}

impl UnsealedTargetNode {
    /// The node, given the sealed arena its attribute values were encoded
    /// into.
    pub fn seal(self, arena: &Arc<CoercedAttrArena>) -> TargetNode {
        TargetNode::new(
            self.rule,
            self.package,
            self.label,
            AttrValues::new(arena.dupe(), self.attributes),
            self.call_stack,
            self.package_cfg_modifiers,
            self.test_config_unification_rollout,
        )
    }
}

impl TargetNode {
    pub fn new(
        rule: Arc<Rule>,
        package: Arc<Package>,
        label: TargetLabel,
        attributes: AttrValues,
        call_stack: Option<StarlarkCallStack>,
        package_cfg_modifiers: Option<CfgModifiers>,
        test_config_unification_rollout: bool,
    ) -> TargetNode {
        TargetNode(triomphe::Arc::new(TargetNodeData {
            rule,
            package,
            label,
            attributes,
            deps_cache: LazyCoercedDeps::default(),
            call_stack,
            package_cfg_modifiers,
            test_config_unification_rollout,
        }))
    }

    /// Testing-only: clone with the given intersection, modelling PACKAGE
    /// evaluation for harnesses without PACKAGE files. Call stack is dropped (debug-only).
    pub fn testing_with_visibility_intersection(
        &self,
        intersection: VisibilityIntersection,
    ) -> TargetNode {
        TargetNode::new(
            self.0.rule.dupe(),
            Arc::new(Package {
                buildfile_path: self.0.package.buildfile_path.dupe(),
                oncall: self.0.package.oncall.dupe(),
                visibility_intersection: intersection,
            }),
            self.0.label.dupe(),
            self.0.attributes.clone(),
            None,
            self.0.package_cfg_modifiers.dupe(),
            self.0.test_config_unification_rollout,
        )
    }

    pub fn rule_kind(&self) -> RuleKind {
        self.0.rule.rule_kind
    }

    pub fn is_configuration_rule(&self) -> bool {
        self.0.rule.rule_kind == RuleKind::Configuration
    }

    pub fn uses_plugins(&self) -> &[PluginKind] {
        self.as_ref().uses_plugins()
    }

    pub fn execution_groups(&self) -> &[RuleExecutionGroup] {
        self.as_ref().execution_groups()
    }

    pub fn default_execution_group(&self) -> bool {
        self.as_ref().default_execution_group()
    }

    pub fn get_default_target_platform(&self) -> buck2_error::Result<Option<&TargetLabel>> {
        Ok(self
            .known_attr_or_none(
                DEFAULT_TARGET_PLATFORM_ATTRIBUTE.id,
                AttrInspectOptions::All,
            )
            .map(|v| v.value.unpack_configuration_dep())
            .transpose()?
            .flatten()
            .map(|t| t.target()))
    }

    #[inline]
    pub fn buildfile_path(&self) -> &BuildFilePath {
        self.as_ref().buildfile_path()
    }

    /// Returns all deps for this node that we know about after processing the build file
    #[inline]
    pub fn deps(&self) -> impl Iterator<Item = &TargetLabel> {
        self.as_ref().deps()
    }

    /// Deps which are to be transitioned to other configuration using transition function.
    #[inline]
    pub fn transition_deps(&self) -> impl Iterator<Item = (&TargetLabel, &Arc<TransitionId>)> {
        self.as_ref().transition_deps()
    }

    pub fn label(&self) -> &TargetLabel {
        &self.0.label
    }

    /// Get the iterator of all special attributes.
    ///
    /// "special" attributes are generated by buck, not provided by users.
    pub fn special_attrs(&self) -> impl Iterator<Item = (&str, SpecialAttr)> {
        self.as_ref().special_attrs()
    }

    /// Get special attribute value if it exists, otherwise return None.
    ///
    /// "special" attributes are generated by buck, not provided by users.
    #[inline]
    pub fn special_attr_or_none(&self, key: &str) -> Option<SpecialAttr> {
        self.as_ref().special_attr_or_none(key)
    }

    pub fn visibility(&self) -> buck2_error::Result<&VisibilitySpecification> {
        match self.0.attributes.get(VISIBILITY_ATTRIBUTE.id) {
            Some(a) => match a.downcast::<kinds::Visibility>() {
                Some(v) => Ok(v),
                _ => {
                    // This code is unreachable: visibility attributes are validated
                    // at the coercion stage. But if we did it wrong,
                    // better error with all the context than panic.
                    Err(internal_error!(
                        "`visibility` attribute coerced incorrectly (`{0}`)",
                        a.as_display_no_ctx(),
                    ))
                }
            },
            None => {
                static DEFAULT: VisibilitySpecification = VisibilitySpecification::DEFAULT;
                Ok(&DEFAULT)
            }
        }
    }

    pub fn is_visible_to(&self, target: &TargetLabel) -> buck2_error::Result<bool> {
        if self.label().pkg() == target.pkg() {
            return Ok(true);
        }
        if !self.visibility()?.0.matches_target(target)? {
            return Ok(false);
        }
        self.0.package.visibility_intersection.matches(target)
    }

    /// Audit-mode helper: true for blocks caused only by `Default` intersection
    /// layers (downgraded to warnings), distinguishing them from own-visibility
    /// and marker-boundary violations (fail in every mode).
    pub fn is_visible_to_ignoring_default_layers(
        &self,
        target: &TargetLabel,
    ) -> buck2_error::Result<bool> {
        if self.label().pkg() == target.pkg() {
            return Ok(true);
        }
        if !self.visibility()?.0.matches_target(target)? {
            return Ok(false);
        }
        self.0
            .package
            .visibility_intersection
            .matches_marker_layers(target)
    }

    pub fn not_visible_to_error(&self, consumer: TargetLabel) -> VisibilityError {
        let intersection = self.0.package.visibility_intersection.dupe();
        if intersection.is_unrestricted() {
            VisibilityError::NotVisibleTo(self.label().dupe(), consumer)
        } else {
            let visibility = self
                .visibility()
                .map(|v| v.dupe())
                .unwrap_or(VisibilitySpecification::DEFAULT);
            VisibilityError::NotVisibleToWithIntersection(
                self.label().dupe(),
                consumer,
                visibility,
                intersection,
            )
        }
    }

    /// Returns an iterator of all attributes.
    ///
    /// "attribute" here is a user defined attribute, not including "special" attributes.
    #[inline]
    pub fn attrs(
        &self,
        opts: AttrInspectOptions,
    ) -> impl ExactSizeIterator<Item = CoercedAttrFull<'_>> {
        self.as_ref().attrs(opts)
    }

    /// Return `None` if attribute is not present or unknown.
    ///
    /// "attribute" here is a user defined attribute, not including "special" attributes.
    #[inline]
    pub fn attr_or_none<'a>(
        &'a self,
        key: &str,
        opts: AttrInspectOptions,
    ) -> Option<CoercedAttrFull<'a>> {
        self.as_ref().attr_or_none(key, opts)
    }

    /// Return `None` if attribute is not present or unknown.
    pub fn known_attr_or_none(
        &self,
        id: AttributeId,
        opts: AttrInspectOptions,
    ) -> Option<CoercedAttrFull<'_>> {
        self.as_ref().known_attr_or_none(id, opts)
    }

    /// Like [`Self::attr_or_none`], but as an owned handle sharing this
    /// target's storage, for holders that outlive the borrow of the node.
    #[inline]
    pub fn owned_attr_or_none(
        &self,
        key: &str,
        opts: AttrInspectOptions,
    ) -> Option<OwnedCoercedAttr> {
        self.as_ref().owned_attr_or_none(key, opts)
    }

    /// Like [`Self::attrs`], but yielding owned handles sharing this
    /// target's storage.
    #[inline]
    pub fn owned_attrs(
        &self,
        opts: AttrInspectOptions,
    ) -> impl Iterator<Item = (&str, OwnedCoercedAttr)> {
        self.as_ref().owned_attrs(opts)
    }

    /// Get attribute.
    ///
    /// * `None` if attribute is known but not set and no default.
    /// * error if attribute is unknown.
    ///
    /// "attribute" here is a user defined attribute, not including "special" attributes.
    #[inline]
    pub fn attr<'a>(
        &'a self,
        key: &str,
        opts: AttrInspectOptions,
    ) -> buck2_error::Result<Option<CoercedAttrFull<'a>>> {
        self.as_ref().attr(key, opts)
    }

    #[inline]
    pub fn target_deps(&self) -> &[TargetLabel] {
        self.as_ref().target_deps()
    }

    #[inline]
    pub fn exec_deps(&self) -> &[TargetLabel] {
        self.as_ref().exec_deps()
    }

    #[inline]
    pub fn get_configuration_deps(&self) -> impl Iterator<Item = &ProvidersLabel> {
        self.as_ref().get_configuration_deps()
    }
    #[inline]
    pub fn toolchain_deps(&self) -> &[TargetLabel] {
        self.as_ref().toolchain_deps()
    }

    pub fn tests(&self) -> impl Iterator<Item = &ProvidersLabel> {
        #[derive(Default)]
        struct TestCollector<'a> {
            labels: Vec<&'a ProvidersLabel>,
        }

        impl<'a> CoercedAttrTraversal<'a> for TestCollector<'a> {
            fn input(&mut self, _path: SourcePathRef) -> buck2_error::Result<()> {
                Ok(())
            }

            fn dep(&mut self, _dep: &ProvidersLabel) -> buck2_error::Result<()> {
                Ok(())
            }

            fn label(&mut self, label: &'a ProvidersLabel) -> buck2_error::Result<()> {
                self.labels.push(label);
                Ok(())
            }
        }

        let tests = self
            .attr_or_none(TESTS_ATTRIBUTE.name, AttrInspectOptions::All)
            .expect("tests is an internal attribute field and will always be present");

        let mut traversal = TestCollector::default();
        tests.traverse(self.label().pkg(), &mut traversal).unwrap();
        traversal.labels.into_iter()
    }

    pub fn inputs(&self) -> impl Iterator<Item = CellPath> + '_ {
        self.as_ref().inputs()
    }

    /// Hash the fields that impact how this target is built.
    /// Don't do any recursive hashing of the dependencies.
    pub fn target_hash<H: Hasher>(&self, state: &mut H) {
        self.label().strong_hash(state);
        self.rule_type().hash(state);
        self.package_cfg_modifiers().hash(state);
        // Hash the visibility intersection so that tightening the visibility
        // at the PACKAGE level still invalidates the target hash
        self.visibility_intersection().hash(state);
        self.attrs(AttrInspectOptions::All).for_each(|x| {
            // We deliberately don't hash the attribute, as if the value being passed to analysis
            // stays the same, we don't care if the attribute that generated it changed.
            x.name.hash(state);
            // The stored identity hash is 128-bit blake3 over full content
            // (`CoercedAttrArena::strong_of`), so folding it preserves the strength
            // of hashing the value itself without re-walking the tree.
            x.value
                .root_strong_hash()
                .expect("should be a root: attr reads serve row and default values")
                .hash(state);
        });
        self.test_config_unification_rollout().hash(state);
    }

    #[inline]
    pub fn metadata(&self) -> buck2_error::Result<Option<&MetadataMap>> {
        self.as_ref().metadata()
    }

    #[inline]
    pub fn as_ref(&self) -> TargetNodeRef<'_> {
        TargetNodeRef(triomphe::Arc::borrow_arc(&self.0))
    }
}

#[derive(Copy, Clone)]
pub struct TargetNodeRef<'a>(triomphe::ArcBorrow<'a, TargetNodeData>);

impl Dupe for TargetNodeRef<'_> {}

impl Deref for TargetNodeRef<'_> {
    type Target = TargetNodeData;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> TargetNodeRef<'a> {
    #[inline]
    pub fn label(self) -> &'a TargetLabel {
        &self.0.get().label
    }

    #[inline]
    pub fn to_owned(self) -> TargetNode {
        TargetNode(triomphe::ArcBorrow::clone_arc(&self.0))
    }

    pub fn buildfile_path(self) -> &'a BuildFilePath {
        &self.0.get().package.buildfile_path
    }

    /// Get attribute.
    ///
    /// * `None` if attribute is known but not set and no default.
    /// * error if attribute is unknown.
    ///
    /// "attribute" here is a user defined attribute, not including "special" attributes.
    pub fn attr(
        self,
        key: &str,
        opts: AttrInspectOptions,
    ) -> buck2_error::Result<Option<CoercedAttrFull<'a>>> {
        self.0
            .get()
            .rule
            .attributes
            .attr(&self.0.get().attributes, key, opts)
    }

    /// Return `None` if attribute is not present or unknown.
    ///
    /// "attribute" here is a user defined attribute, not including "special" attributes.
    pub fn attr_or_none(&self, key: &str, opts: AttrInspectOptions) -> Option<CoercedAttrFull<'a>> {
        self.0
            .get()
            .rule
            .attributes
            .attr_or_none(&self.0.get().attributes, key, opts)
    }

    /// Return `None` if attribute is not present or unknown.
    pub fn known_attr_or_none(
        &self,
        id: AttributeId,
        opts: AttrInspectOptions,
    ) -> Option<CoercedAttrFull<'a>> {
        self.0
            .get()
            .rule
            .attributes
            .known_attr_or_none(id, &self.0.get().attributes, opts)
    }

    /// Get the iterator of all attributes.
    ///
    /// "attribute" here is a user defined attribute, not including "special" attributes.
    pub fn attrs(
        self,
        opts: AttrInspectOptions,
    ) -> impl ExactSizeIterator<Item = CoercedAttrFull<'a>> {
        self.0
            .get()
            .rule
            .attributes
            .attrs(&self.0.get().attributes, opts)
    }

    /// Like [`Self::attr_or_none`], but as an owned handle sharing this
    /// target's storage, for holders that outlive the borrow of the node.
    pub fn owned_attr_or_none(
        &self,
        key: &str,
        opts: AttrInspectOptions,
    ) -> Option<OwnedCoercedAttr> {
        self.0
            .get()
            .rule
            .attributes
            .owned_attr_or_none(&self.0.get().attributes, key, opts)
    }

    /// Like [`Self::attrs`], but yielding owned handles sharing this
    /// target's storage.
    pub fn owned_attrs(
        self,
        opts: AttrInspectOptions,
    ) -> impl Iterator<Item = (&'a str, OwnedCoercedAttr)> + 'a {
        self.0
            .get()
            .rule
            .attributes
            .owned_attrs(&self.0.get().attributes, opts)
    }

    /// Get special attribute value if it exists, otherwise return None.
    ///
    /// "special" attributes are generated by buck, not provided by users.
    pub fn special_attr_or_none(self, key: &str) -> Option<SpecialAttr> {
        match key {
            TYPE => Some(SpecialAttr::String(self.rule_type().name().into())),
            CONFIGURATION_DEPS => Some(SpecialAttr::ConfigurationDepList(
                self.get_configuration_deps().map(|t| t.dupe()).collect(),
            )),
            DEPS => Some(SpecialAttr::LabelList(
                self.deps()
                    .map(|t| ProvidersLabel::default_for(t.dupe()))
                    .collect(),
            )),
            PACKAGE => Some(SpecialAttr::String(ArcStr::from(
                self.buildfile_path().to_string().as_str(),
            ))),
            ONCALL => match self.oncall() {
                None => Some(SpecialAttr::None),
                Some(x) => Some(SpecialAttr::String(ArcStr::from(x))),
            },
            PACKAGE_CFG_MODIFIERS => match self.package_cfg_modifiers() {
                Some(x) => Some(SpecialAttr::TargetModifiers(*x)),
                None => Some(SpecialAttr::None),
            },
            _ => None,
        }
    }

    /// Get the iterator of all special attributes.
    ///
    /// "special" attributes are generated by buck, not provided by users.
    pub fn special_attrs(self) -> impl Iterator<Item = (&'a str, SpecialAttr)> + 'a {
        [
            TYPE,
            CONFIGURATION_DEPS,
            DEPS,
            PACKAGE,
            ONCALL,
            PACKAGE_CFG_MODIFIERS,
        ]
        .into_iter()
        .map(move |x|
                // use unwrap here, if fail here it means we iter over a key not in the match list from `special_attr_or_none`
                (x, self.special_attr_or_none(x).unwrap()))
    }

    pub fn metadata(self) -> buck2_error::Result<Option<&'a MetadataMap>> {
        self.known_attr_or_none(METADATA_ATTRIBUTE.id, AttrInspectOptions::All)
            .map(|attr| match attr.value.downcast::<kinds::Metadata>() {
                Some(m) => Ok(m),
                x => Err(internal_error!("`metadata` attribute should be coerced as a dict of strings to JSON values. Found `{:?}` instead", x)),
            })
            .transpose()
    }

    /// The target's own `modifiers`, shared with the stored value.
    pub fn target_modifiers(self) -> buck2_error::Result<Option<CfgModifiers>> {
        self.known_attr_or_none(TARGET_MODIFIERS_ATTRIBUTE.id, AttrInspectOptions::All)
            .map(|attr| match attr.value.downcast::<kinds::Modifiers>() {
                Some(m) => Ok(m.dupe()),
                x => Err(internal_error!(
                    "`modifiers` attribute should be coerced as modifiers. Found `{:?}` instead",
                    x
                )),
            })
            .transpose()
    }

    pub fn target_deps(self) -> &'a [TargetLabel] {
        &self.0.get().deps_cache().deps
    }

    pub fn exec_deps(self) -> &'a [TargetLabel] {
        &self.0.get().deps_cache().exec_deps
    }

    pub fn toolchain_deps(self) -> &'a [TargetLabel] {
        &self.0.get().deps_cache().toolchain_deps
    }

    pub fn get_configuration_deps(self) -> impl Iterator<Item = &'a ProvidersLabel> {
        self.get_configuration_deps_with_kind().map(|(d, _)| d)
    }

    pub fn get_configuration_deps_with_kind(
        self,
    ) -> impl Iterator<Item = (&'a ProvidersLabel, ConfigurationDepKind)> {
        self.0
            .get()
            .deps_cache()
            .configuration_deps
            .iter()
            .map(|(d, k)| (d, *k))
    }

    /// Returns all deps for this node that we know about after processing the build file
    pub fn deps(self) -> impl Iterator<Item = &'a TargetLabel> {
        let deps_cache = self.0.get().deps_cache();
        deps_cache
            .deps
            .iter()
            .chain(deps_cache.transition_deps.iter().map(|(dep, _tr)| dep))
            .chain(deps_cache.exec_deps.iter())
            .chain(deps_cache.toolchain_deps.iter())
            .chain(deps_cache.plugin_deps.iter())
    }

    /// Deps which are to be transitioned to other configuration using transition function.
    pub fn transition_deps(self) -> impl Iterator<Item = (&'a TargetLabel, &'a Arc<TransitionId>)> {
        self.0
            .get()
            .deps_cache()
            .transition_deps
            .iter()
            .map(|x| (&x.0, &x.1))
    }

    pub fn uses_plugins(self) -> &'a [PluginKind] {
        &self.0.get().rule.uses_plugins
    }

    pub fn execution_groups(self) -> &'a [RuleExecutionGroup] {
        &self.0.get().rule.execution_groups
    }

    /// The attribute whose constraints only the default execution group's platform must satisfy.
    pub fn default_exec_compatible_with_attr(self) -> Option<AttributeId> {
        self.0.get().rule.default_exec_compatible_with_attr
    }

    pub fn default_execution_group(self) -> bool {
        self.0.get().rule.default_execution_group
    }

    pub fn inputs(self) -> impl Iterator<Item = CellPath> + 'a {
        struct InputsCollector {
            inputs: Vec<CellPath>,
        }

        impl CoercedAttrTraversal<'_> for InputsCollector {
            fn input(&mut self, path: SourcePathRef) -> buck2_error::Result<()> {
                self.inputs.push(path.to_cell_path());
                Ok(())
            }

            fn dep(&mut self, _dep: &ProvidersLabel) -> buck2_error::Result<()> {
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
}

pub mod testing {
    use buck2_core::package::PackageLabel;
    use buck2_fs::paths::file_name::FileNameBuf;
    use serde_json::map::Map;
    use serde_json::value::Value;

    use super::*;
    use crate::attrs::attr::Attribute;
    use crate::attrs::fmt_context::AttrFmtContext;
    use crate::attrs::spec::internal::NAME_ATTRIBUTE;
    use crate::nodes::targets_map::TargetsMap;
    use crate::rule::RuleIncomingTransition;

    pub trait TargetNodeExt {
        /// Does not populate `deps_cache`, so reading deps from the resulting node exercises
        /// the same lazy initializer as production. That initializer traverses the rule's full
        /// attribute spec, so the deps include those contributed by default attribute values,
        /// not only by `attrs`.
        fn testing_new(
            label: TargetLabel,
            rule_type: RuleType,
            attrs: Vec<(&str, Attribute, OwnedCoercedAttr)>,
            call_stack: Option<StarlarkCallStack>,
        ) -> Self;
    }

    impl TargetNodeExt for TargetNode {
        fn testing_new(
            label: TargetLabel,
            rule_type: RuleType,
            attrs: Vec<(&str, Attribute, OwnedCoercedAttr)>,
            call_stack: Option<StarlarkCallStack>,
        ) -> TargetNode {
            let attr_spec = AttributeSpec::testing_new(
                attrs
                    .iter()
                    .map(|(name, attr, _)| ((*name).to_owned(), attr.clone()))
                    .collect(),
            );

            let mut builder = CoercedAttrArena::builder();
            let mut attributes = UnsealedAttrValues::with_capacity(attrs.len() + 1);

            let name = builder.encode_string(label.name().as_str()).unwrap();
            attributes.push(&mut builder, NAME_ATTRIBUTE.id, name);

            for (name, _attr, val) in attrs.into_iter() {
                let idx = attr_spec.attribute_id_by_name(name).unwrap();
                let val = builder.encode_view(val.as_ref()).unwrap();
                attributes.push(&mut builder, idx, val);
            }

            let attributes = AttrValues::new(Arc::new(builder.finish()), attributes);

            let buildfile_path = Arc::new(BuildFilePath::new(
                label.pkg().dupe(),
                FileNameBuf::unchecked_new("BUCK"),
            ));
            TargetNode::new(
                Arc::new(Rule {
                    attributes: attr_spec,
                    rule_type,
                    rule_kind: RuleKind::Normal,
                    cfg: RuleIncomingTransition::None,
                    uses_plugins: Vec::new(),
                    execution_groups: Vec::new(),
                    default_execution_group: true,
                    default_exec_compatible_with_attr: None,
                }),
                Arc::new(Package {
                    buildfile_path,
                    oncall: None,
                    visibility_intersection: VisibilityIntersection::unrestricted(),
                }),
                label,
                attributes,
                call_stack,
                None,
                false,
            )
        }
    }

    /// Take a TargetsMap and convert it to a nice json representation. Adds in a __type__ attr
    /// for each target's values to make it clear what the rule type is. That can probably go
    /// away eventually.
    pub fn targets_to_json(
        target: &TargetsMap,
        pkg: PackageLabel,
        opts: AttrInspectOptions,
    ) -> buck2_error::Result<Value> {
        let map: Map<String, Value> = target
            .iter()
            .map(|(target_name, values)| {
                let mut json_values: Map<String, Value> = values
                    .attrs(opts)
                    .map(|a| {
                        Ok((
                            a.name.to_owned(),
                            a.value.to_json(&AttrFmtContext {
                                package: Some(pkg.dupe()),
                                options: Default::default(),
                            })?,
                        ))
                    })
                    .collect::<buck2_error::Result<Map<String, Value>>>()?;
                json_values.insert(
                    "__type__".to_owned(),
                    Value::String(values.rule_type().to_string()),
                );
                Ok((target_name.to_string(), Value::from(json_values)))
            })
            .collect::<buck2_error::Result<Map<String, Value>>>()?;
        Ok(Value::from(map))
    }
}
