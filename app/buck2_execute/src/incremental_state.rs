/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! What an incremental action, one run with `no_outputs_cleanup`, starts its next run from: the
//! outputs its last run left, as a record kept per action, rather than whatever its paths happen
//! to hold. The action writes the record from `buck2_action_impl`, the executors read it in
//! `buck2_execute_impl`, where the sqlite database behind the store lives too, and the daemon
//! attaches the store in `buck2_server`, so the types are here, as the dep-file cache's are.

use std::sync::Arc;

use allocative::Allocative;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckFixedSortedMap;
use dice::UserComputationData;
use dupe::Dupe;

use crate::artifact_value::ArtifactValue;
use crate::execute::kind::CommandExecutionKind;
use crate::execute::request::CommandExecutionOutput;
use crate::execute::result::CommandExecutionResult;
use crate::execute::result::CommandExecutionStatus;

/// An incremental action's record, keyed in the store by the action
/// (`RunActionKey::incremental_state_key`).
#[derive(Debug, Allocative)]
pub enum IncrementalRecord {
    /// The outputs the last result had, by their path within the action's output directory: what
    /// the next run starts from, all of them or none.
    Outputs(BuckFixedSortedMap<ForwardRelativePathBuf, RecordedOutput>),
    /// The last local run did not succeed, and its outputs are whatever it left at its paths. The
    /// next run starts from those, as a tool expects after its own failure; nothing vouches for
    /// them.
    Leftovers,
}

impl IncrementalRecord {
    /// What `result` leaves for the action's next run: a success's outputs; a local run's
    /// failure as leftovers, since the run wrote at its paths and what it left is what the next
    /// run finds; nothing for a failure elsewhere, which changed nothing at the paths.
    pub fn of_result(result: &CommandExecutionResult) -> Option<Self> {
        match &result.report.status {
            CommandExecutionStatus::Success { .. } => Some(Self::Outputs(
                result
                    .outputs
                    .iter()
                    .filter_map(|(output, value)| match output {
                        CommandExecutionOutput::BuildArtifact { path, .. } => {
                            Some((path.path().to_buf(), RecordedOutput::Value(value.dupe())))
                        }
                        CommandExecutionOutput::TestPath { .. } => None,
                    })
                    .collect(),
            )),
            status => match status.execution_kind() {
                Some(
                    CommandExecutionKind::Local { .. }
                    | CommandExecutionKind::LocalWorker { .. }
                    | CommandExecutionKind::LocalWorkerInit { .. },
                ) => Some(Self::Leftovers),
                _ => None,
            },
        }
    }
}

/// One output of a record.
#[derive(Debug, Clone, Dupe, Allocative)]
pub enum RecordedOutput {
    /// The value, whole. It holds the value's tracked digests, so that the stash keeps a locally
    /// made output's blobs for as long as the record does.
    Value(ArtifactValue),
    /// A directory reloaded from the database, known by its fingerprint alone: the tree is the
    /// materializer's, which has it whole if it still tracks it.
    DirectoryFingerprint(FileDigest),
}

impl RecordedOutput {
    /// The hash this output's content-based path resolves with.
    pub fn content_based_path_hash(&self) -> buck2_error::Result<ContentBasedPathHash> {
        match self {
            // An action's own output carries an inferred hash, the digest's; an explicit one is
            // set only on projected artifacts, which no action declares.
            Self::Value(value) => Ok(value.content_based_path_hash()),
            Self::DirectoryFingerprint(fingerprint) => {
                ContentBasedPathHash::new(fingerprint.raw_digest().as_bytes())
            }
        }
    }
}

/// The incremental records, one per action.
pub trait IncrementalStore: Send + Sync + 'static {
    fn get(&self, key: &str) -> Option<Arc<IncrementalRecord>>;

    /// Replaces the action's record.
    fn insert(&self, key: &str, record: IncrementalRecord);
}

struct IncrementalStoreHolder(Option<Arc<dyn IncrementalStore>>);

pub trait SetIncrementalStore {
    fn set_incremental_store(&mut self, store: Option<Arc<dyn IncrementalStore>>);
}

pub trait HasIncrementalStore {
    fn get_incremental_store(&self) -> Option<&dyn IncrementalStore>;
}

impl SetIncrementalStore for UserComputationData {
    fn set_incremental_store(&mut self, store: Option<Arc<dyn IncrementalStore>>) {
        self.data.set(IncrementalStoreHolder(store));
    }
}

impl HasIncrementalStore for UserComputationData {
    fn get_incremental_store(&self) -> Option<&dyn IncrementalStore> {
        self.data
            .get::<IncrementalStoreHolder>()
            .ok()
            .and_then(|holder| holder.0.as_deref())
    }
}
