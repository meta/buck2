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
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_error::BuckErrorOptionContext;
use buck2_execute::materialize::materializer::ReadLease;
use buck2_hash::BuckDashMap;
use buck2_test_api::data::ConfiguredTargetHandle;
use dupe::Dupe;
use pagable::Pagable;

#[derive(
    Debug, Clone, Copy, Dupe, Default, Allocative, PartialEq, Hash, Eq, Pagable
)]
pub struct TestSessionOptions {
    /// Whether this session should allow things to run on RE.
    pub allow_re: bool,
    pub force_use_project_relative_paths: bool,
    pub force_run_from_project_root: bool,
}

impl fmt::Display for TestSessionOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "allow_re = {}, force_use_project_relative_paths = {}, force_run_from_project_root = {}",
            self.allow_re, self.force_use_project_relative_paths, self.force_run_from_project_root
        )
    }
}

/// The state of a buck2 test command.
pub struct TestSession {
    /// The next ConfiguredTargetHandle that will be assigned.
    next_id: AtomicU64,
    /// A mapping of ConfiguredTargetHandle (which Tpx can use with) to the underlying provider in
    /// Buck2.
    labels: BuckDashMap<ConfiguredTargetHandle, ConfiguredProvidersLabel>,
    /// Options overriding the behavior of tests executed in this session. This is primarily
    /// intended for unstable or debugging features.
    options: TestSessionOptions,
    /// Leases over what the test runner reads out of process, by the output root of the
    /// execution they belong to: a run's outputs, or the inputs of a command the runner executes
    /// itself. Each is held until the runner comes back to its root or the session ends.
    held: BuckDashMap<ProjectRelativePathBuf, ReadLease>,
}

impl TestSession {
    pub fn new(options: TestSessionOptions) -> Self {
        Self {
            next_id: AtomicU64::new(0),
            labels: BuckDashMap::default(),
            options,
            held: BuckDashMap::default(),
        }
    }

    /// Holds `lease` for the execution at `root`, in place of whatever was held for it.
    pub fn hold(&self, root: ProjectRelativePathBuf, lease: ReadLease) {
        self.held.insert(root, lease);
    }

    /// Releases what is held for the execution at `root`, ahead of producing there again.
    pub fn release(&self, root: &ProjectRelativePath) {
        self.held.remove(root);
    }

    /// Releases everything held: the runner is done reading.
    pub fn release_all(&self) {
        self.held.clear();
    }

    pub fn options(&self) -> TestSessionOptions {
        self.options
    }

    /// Insert a new provider and retrieve the matching handle.
    pub fn register(&self, label: ConfiguredProvidersLabel) -> ConfiguredTargetHandle {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).into();
        let inserted = self.labels.insert(id, label).is_none();
        assert!(inserted);
        id
    }

    /// Retrieve the provider for a given handle.
    pub fn get(&self, id: ConfiguredTargetHandle) -> buck2_error::Result<ConfiguredProvidersLabel> {
        let res = self
            .labels
            .get(&id)
            .with_internal_error(|| format!("Invalid id provided to TestSession: {id:?}"))?;

        Ok(res.clone())
    }
}
