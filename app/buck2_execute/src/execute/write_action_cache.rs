/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The action cache as write actions use it (`buck2.write_action_cache`): one entry per write,
//! under a synthetic action whose command names the write's key, holding the content's digest
//! as its one output. Entries are read and recorded under the executor configuration's use
//! case and platform, as a run action's are.

use async_trait::async_trait;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use remote_execution::TActionResult2;

use crate::digest_config::DigestConfig;
use crate::execute::action_digest_and_blobs::ActionDigestAndBlobs;

#[async_trait]
pub trait WriteActionCache: Send + Sync {
    /// The use case entries are read and recorded under and content is uploaded as, which is
    /// what a hit's content is declared with.
    fn use_case(&self) -> RemoteExecutorUseCase;

    /// Whether `populate` may be called: the executor configuration publishes local results
    /// (`allow_cache_uploads`), and a write's content recorded for other builds is one.
    fn populates(&self) -> bool;

    /// The entry under `action`, if there is one. `Err` when the cache could not answer.
    async fn lookup(
        &self,
        action: &ActionDigestAndBlobs,
    ) -> buck2_error::Result<Option<TActionResult2>>;

    /// Uploads `content`, whose digest is `digest`, to the CAS unless the CAS holds it, for a
    /// write whose content is declared as CAS content without an entry.
    async fn upload(
        &self,
        content: &[u8],
        digest: &TrackedFileDigest,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<()>;

    /// Records `content`, whose digest is `digest`, as the one output of `action`: uploads it
    /// and the action's blobs, whichever the CAS lacks, and writes the entry. `digest` carries the entry's expiration
    /// afterwards, a lower bound on the content's. Only where `populates`.
    async fn populate(
        &self,
        action: &ActionDigestAndBlobs,
        content: &[u8],
        digest: &TrackedFileDigest,
        is_executable: bool,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<PopulateOutcome>;
}

pub enum PopulateOutcome {
    Populated,
    /// The invocation may not write the action cache, which was found out before anything was
    /// uploaded.
    PermissionDenied {
        reason: String,
    },
}
