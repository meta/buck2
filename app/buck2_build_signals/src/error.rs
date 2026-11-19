/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use tokio::task::JoinError;

/// We consider buck's critical path computation to be a core feature of buck and so
/// treat failures severely, but logically the command results don't really depend on it and
/// so failing a build on a spurious critical path computation failure is a high cost.
///
/// Because of this, we are careful about exactly what errors may be produced from the critical
/// path build listeners rather than just propagating buck2_error::Results around.
#[derive(buck2_error::Error, Debug)]
#[buck2(tag = CriticalPathError)]
pub enum CriticalPathError {
    #[error("Overflow building critical path graph graph")]
    GraphBuildOverflow,
    #[error("Critical path graph has a cycle: {0}")]
    CycleDetected(String),
    #[error("Critical path task was cancelled (e.g. daemon shutdown or command preemption): {0:?}")]
    JoinError(JoinError),
}
