/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_hash::BuckMutInsertionSet;

use crate::attrs::configured_traversal::ConfiguredAttrTraversal;

#[derive(Default, Debug)]
pub struct ConfiguredAttrInfoForTests {
    // Including transitioned deps.
    pub deps: BuckMutInsertionSet<ConfiguredProvidersLabel>,
    pub execution_deps: BuckMutInsertionSet<ConfiguredProvidersLabel>,
    pub toolchain_deps: BuckMutInsertionSet<ConfiguredProvidersLabel>,
}

impl ConfiguredAttrInfoForTests {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ConfiguredAttrTraversal for ConfiguredAttrInfoForTests {
    fn dep(&mut self, dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
        self.deps.insert(dep.clone());
        Ok(())
    }

    fn exec_dep(&mut self, dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
        self.execution_deps.insert(dep.clone());
        Ok(())
    }

    fn toolchain_dep(&mut self, dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
        self.toolchain_deps.insert(dep.clone());
        Ok(())
    }
}
