/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::ops::Deref;

use allocative::Allocative;
use buck2_core::build_file_path::BuildFilePath;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_query::query::environment::QueryTarget;
use buck2_query::query::graph::node::LabeledNode;
use buck2_query::query::graph::node::NodeKey;
use dupe::Dupe;
use pagable::Pagable;
use ref_cast::RefCast;

use crate::attrs::attr_type::any_matches::AnyMatches;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::inspect_options::AttrInspectOptions;
use crate::nodes::configured::ConfiguredTargetNode;

/// `ConfiguredTargetNode` as both `LabeledNode` and `NodeLabel` and also `QueryTarget`.
#[derive(Debug, Dupe, Clone, RefCast, Allocative, Pagable)]
#[repr(C)]
pub struct ConfiguredGraphNodeRef(ConfiguredTargetNode);

impl NodeKey for ConfiguredGraphNodeRef {}

impl Deref for ConfiguredGraphNodeRef {
    type Target = ConfiguredTargetNode;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ConfiguredGraphNodeRef {
    #[inline]
    pub fn new(node: ConfiguredTargetNode) -> Self {
        ConfiguredGraphNodeRef(node)
    }

    pub fn label(&self) -> &ConfiguredTargetLabel {
        self.0.label()
    }

    #[inline]
    pub fn into_inner(self) -> ConfiguredTargetNode {
        self.0
    }
}

impl std::fmt::Display for ConfiguredGraphNodeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.label().fmt(f)
    }
}

impl PartialOrd for ConfiguredGraphNodeRef {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ConfiguredGraphNodeRef {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.label().cmp(other.label())
    }
}

impl PartialEq for ConfiguredGraphNodeRef {
    fn eq(&self, other: &Self) -> bool {
        // `ptr_eq` is optimization.
        self.0.ptr_eq(&other.0) || self.label().eq(other.label())
    }
}

impl Eq for ConfiguredGraphNodeRef {}

impl std::hash::Hash for ConfiguredGraphNodeRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hashed_label().hash().hash(state);
    }
}

impl LabeledNode for ConfiguredGraphNodeRef {
    type Key = ConfiguredGraphNodeRef;

    fn node_key(&self) -> &Self::Key {
        self
    }
}

impl QueryTarget for ConfiguredGraphNodeRef {
    type Attr<'a> = ConfiguredAttr;

    fn label_for_filter(&self) -> String {
        self.0.label().unconfigured().to_string()
    }

    fn rule_type(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.0.rule_type().name())
    }

    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.0.label().name().as_str())
    }

    fn buildfile_path(&self) -> buck2_error::Result<&BuildFilePath> {
        Ok(self.0.body()?.buildfile_path())
    }

    fn deps(&self) -> impl Iterator<Item = &Self::Key> + Send + '_ {
        self.0.deps().iter().map(ConfiguredGraphNodeRef::ref_cast)
    }

    fn exec_deps(&self) -> impl Iterator<Item = &Self::Key> + Send + '_ {
        self.0
            .exec_deps()
            .iter()
            .map(ConfiguredGraphNodeRef::ref_cast)
    }

    fn target_deps(&self) -> impl Iterator<Item = &Self::Key> + Send + '_ {
        self.0.target_deps().map(ConfiguredGraphNodeRef::ref_cast)
    }

    fn configuration_deps(&self) -> impl Iterator<Item = &Self::Key> + Send + '_ {
        self.0
            .configuration_deps()
            .map(ConfiguredGraphNodeRef::ref_cast)
    }

    fn toolchain_deps(&self) -> impl Iterator<Item = &Self::Key> + Send + '_ {
        self.0
            .toolchain_deps()
            .map(ConfiguredGraphNodeRef::ref_cast)
    }
    fn attr_any_matches(
        attr: &Self::Attr<'_>,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        attr.any_matches(filter)
    }

    fn attr_matches(
        &self,
        key: &str,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        self.0.body()?.attr_matches(key, filter)
    }

    fn special_attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        mut func: F,
    ) -> Result<(), E> {
        for (name, attr) in self.0.body()?.special_attrs() {
            func(name, &attr)?;
        }
        Ok(())
    }

    fn attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        mut func: F,
    ) -> Result<(), E> {
        for a in self.0.body()?.attrs(AttrInspectOptions::All) {
            func(a.name, &a.value)?;
        }
        Ok(())
    }

    fn defined_attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        mut func: F,
    ) -> Result<(), E> {
        for a in self.0.body()?.attrs(AttrInspectOptions::DefinedOnly) {
            func(a.name, &a.value)?;
        }
        Ok(())
    }

    fn map_attr<R, F: FnMut(Option<&Self::Attr<'_>>) -> R>(
        &self,
        key: &str,
        mut func: F,
    ) -> buck2_error::Result<R> {
        Ok(func(
            self.0
                .body()?
                .get(key, AttrInspectOptions::All)
                .as_ref()
                .map(|a| &a.value),
        ))
    }

    fn inputs_for_each<E: From<buck2_error::Error>, F: FnMut(CellPath) -> Result<(), E>>(
        &self,
        mut func: F,
    ) -> Result<(), E> {
        for input in self.0.body()?.inputs() {
            func(input)?;
        }
        Ok(())
    }

    fn map_any_attr<R, F: FnMut(Option<&Self::Attr<'_>>) -> R>(
        &self,
        key: &str,
        mut func: F,
    ) -> buck2_error::Result<R> {
        let body = self.0.body()?;
        Ok(
            match body
                .get(key, AttrInspectOptions::All)
                .as_ref()
                .map(|a| &a.value)
            {
                Some(attr) => func(Some(attr)),
                None => func(body.special_attr_or_none(key).as_ref()),
            },
        )
    }
}
