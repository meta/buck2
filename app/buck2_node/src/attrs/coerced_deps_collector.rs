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
use buck2_core::package::source_path::SourcePathRef;
use buck2_core::plugins::PluginKind;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckMutInsertionSet;
use buck2_util::vec2::Vec2;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;

use crate::attrs::attr_type::configuration_dep::ConfigurationDepKind;
use crate::attrs::traversal::CoercedAttrTraversal;

#[derive(Default, Debug, PartialEq, Eq, Hash, Allocative, Pagable)]
pub struct CoercedDeps {
    /// Contains the deps derived from the attributes.
    /// Does not include the transition, exec or configuration deps.
    pub deps: MiniBoxSlice<TargetLabel>,

    /// Contains the deps which are transitioned to other configuration
    /// (including split transitions).
    pub transition_deps: MiniBoxSlice<(TargetLabel, Arc<TransitionId>)>,

    /// Contains the execution deps derived from the attributes.
    pub exec_deps: MiniBoxSlice<TargetLabel>,

    /// Contains the toolchain deps derived from the attributes.
    pub toolchain_deps: MiniBoxSlice<TargetLabel>,

    /// Contains the configuration deps
    pub configuration_deps: Vec2<ProvidersLabel, ConfigurationDepKind>,

    /// Contains the plugin deps
    pub plugin_deps: MiniBoxSlice<TargetLabel>,
}

impl From<CoercedDepsCollector> for CoercedDeps {
    fn from(collector: CoercedDepsCollector) -> CoercedDeps {
        let CoercedDepsCollector {
            deps,
            transition_deps,
            exec_deps,
            toolchain_deps,
            configuration_deps,
            plugin_deps,
        } = collector;
        CoercedDeps {
            deps: deps.into_iter().collect(),
            transition_deps: transition_deps.into_iter().collect(),
            exec_deps: exec_deps.into_iter().collect(),
            toolchain_deps: toolchain_deps.into_iter().collect(),
            configuration_deps: configuration_deps.into_iter().collect(),
            plugin_deps: plugin_deps.into_iter().collect(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Allocative)]
pub struct CoercedDepsCollector {
    /// Contains the deps derived from the attributes.
    /// Does not include the transition, exec or configuration deps.
    pub deps: BuckMutInsertionSet<TargetLabel>,

    /// Contains the deps which are transitioned to other configuration
    /// (including split transitions).
    pub transition_deps: BuckMutInsertionSet<(TargetLabel, Arc<TransitionId>)>,

    /// Contains the execution deps derived from the attributes.
    pub exec_deps: BuckMutInsertionSet<TargetLabel>,

    /// Contains the toolchain deps derived from the attributes.
    pub toolchain_deps: BuckMutInsertionSet<TargetLabel>,

    /// Contains the configuration deps. These are deps that appear as conditions in selects.
    pub configuration_deps: BuckMutInsertionSet<(ProvidersLabel, ConfigurationDepKind)>,

    /// Contains the plugin deps
    pub plugin_deps: BuckMutInsertionSet<TargetLabel>,
}

impl CoercedDepsCollector {
    pub fn new() -> Self {
        Self {
            deps: BuckMutInsertionSet::new(),
            exec_deps: BuckMutInsertionSet::new(),
            toolchain_deps: BuckMutInsertionSet::new(),
            transition_deps: BuckMutInsertionSet::new(),
            configuration_deps: BuckMutInsertionSet::new(),
            plugin_deps: BuckMutInsertionSet::new(),
        }
    }
}

impl<'a> CoercedAttrTraversal<'a> for CoercedDepsCollector {
    fn dep(&mut self, dep: &ProvidersLabel) -> buck2_error::Result<()> {
        self.deps.insert(dep.target().dupe());
        Ok(())
    }

    fn exec_dep(&mut self, dep: &'a ProvidersLabel) -> buck2_error::Result<()> {
        self.exec_deps.insert(dep.target().dupe());
        Ok(())
    }

    fn toolchain_dep(&mut self, dep: &'a ProvidersLabel) -> buck2_error::Result<()> {
        self.toolchain_deps.insert(dep.target().dupe());
        Ok(())
    }

    fn transition_dep(
        &mut self,
        dep: &'a ProvidersLabel,
        tr: &Arc<TransitionId>,
    ) -> buck2_error::Result<()> {
        self.transition_deps
            .insert((dep.target().dupe(), tr.dupe()));
        Ok(())
    }

    fn split_transition_dep(
        &mut self,
        dep: &'a ProvidersLabel,
        tr: &Arc<TransitionId>,
    ) -> buck2_error::Result<()> {
        self.transition_deps
            .insert((dep.target().dupe(), tr.dupe()));
        Ok(())
    }

    fn configuration_dep(
        &mut self,
        dep: &ProvidersLabel,
        t: ConfigurationDepKind,
    ) -> buck2_error::Result<()> {
        self.configuration_deps.insert((dep.dupe(), t));
        Ok(())
    }

    fn plugin_dep(&mut self, dep: &'a TargetLabel, _kind: &PluginKind) -> buck2_error::Result<()> {
        self.plugin_deps.insert(dep.dupe());
        Ok(())
    }

    fn input(&mut self, _input: SourcePathRef) -> buck2_error::Result<()> {
        Ok(())
    }
}
