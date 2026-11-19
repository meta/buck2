/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;

use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::internal_error;
use dice_futures::cancellation::CancellationContext;
use dice_futures::cancellation::CancellationHandle;
use dice_futures::spawn::CancellableJoinHandle;
use dice_futures::spawn::WeakFutureError;
use dice_futures::spawn::spawn_dropcancel;
use dice_futures::spawner::TokioSpawner;
use futures::future::BoxFuture;

use crate::materializers::deferred::SharedMaterializingError;
use crate::materializers::deferred::artifact_tree::ArtifactMetadata;

/// The IO that puts one root on disk, running in a task of its own so that it outlives the
/// request that started it. A holder that goes away while its task runs hands the task back with
/// its root, and the next holder of the root either takes the download over or cancels it and
/// waits for it to end before writing there itself. The task does IO only; whoever sees it to its
/// end reports the root.
pub(crate) struct Task {
    root: ProjectRelativePathBuf,
    entry: ArtifactMetadata,
    handle: CancellableJoinHandle<Result<(), SharedMaterializingError>>,
    /// `None` once cancelled.
    cancel: Option<CancellationHandle>,
}

/// How a task ended.
pub(crate) enum Outcome {
    Done(Result<(), SharedMaterializingError>),
    /// Cancelled before it started.
    Cancelled,
}

impl Task {
    /// Starts `io` at once. `io` decides where cancellation may land, and does not let it once it
    /// touches disk, so what it puts there is never half-written.
    pub(crate) fn spawn<F>(root: ProjectRelativePathBuf, entry: ArtifactMetadata, io: F) -> Self
    where
        F: for<'a> FnOnce(
                &'a CancellationContext,
            ) -> BoxFuture<'a, Result<(), SharedMaterializingError>>
            + Send,
    {
        let (handle, cancel) = spawn_dropcancel(io, &TokioSpawner, &()).detach();
        Self {
            root,
            entry,
            handle,
            cancel: Some(cancel),
        }
    }

    pub(crate) fn root(&self) -> &ProjectRelativePath {
        &self.root
    }

    /// What the task puts at its root.
    pub(crate) fn entry(&self) -> &ArtifactMetadata {
        &self.entry
    }

    pub(crate) fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.is_none()
    }

    /// Waits for the task to end. To be called once.
    pub(crate) async fn settle(&mut self) -> Outcome {
        match (&mut self.handle).await {
            Ok(result) => Outcome::Done(result),
            Err(WeakFutureError::Cancelled) => Outcome::Cancelled,
            Err(WeakFutureError::JoinError) => Outcome::Done(Err(SharedMaterializingError::Error(
                internal_error!("A materialization task panicked"),
            ))),
        }
    }
}

impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Task")
            .field("root", &self.root)
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}
