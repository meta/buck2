/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_hash::BuckMutMap;
use starlark::debug::VariablePath;

/// Maps variable IDs to their access paths for tree-structured DAP protocol variables.
///
/// In the DAP protocol, variables are structured as trees. When a variable with a non-zero ID is encountered,
/// it's expected to have child variables. This struct helps maintain a relationship between variable IDs and their
/// access paths to facilitate evaluation and traversal.
///
/// # Example
///
/// A variable with path "name.field1" that has a child "innerField" will be mapped such that
/// the child's ID corresponds to the access path "name.field1.innerField". This allows correct traversal
/// upon a variable request against the child's ID.
#[derive(Default, Debug)]
pub(crate) struct VariablesKnownPaths {
    path_by_id: BuckMutMap<u32, VariablePath>,
}

impl VariablesKnownPaths {
    pub fn get(&self, id: u32) -> Option<&VariablePath> {
        self.path_by_id.get(&id)
    }

    pub fn insert(&mut self, path: VariablePath) -> u32 {
        let id = (self.path_by_id.len() + 1) as u32;
        self.path_by_id.insert(id, path);
        id
    }
}
