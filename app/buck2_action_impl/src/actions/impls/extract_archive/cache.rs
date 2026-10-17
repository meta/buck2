/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;

use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::ActionExecutionCtx;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::dep_file_state::StoredDepFileState;
use buck2_execute::dep_file_state::StoredOutput;
use buck2_execute::dep_file_state::StoredOutputValue;
use buck2_fs::fs_util;
use buck2_util::strong_hasher::Blake3StrongHasher;
use dupe::Dupe;
use regex::Regex;

use crate::actions::impls::extract_archive::unpack::ArchiveType;

pub(super) struct ExtractionCache {
    key: Vec<u8>,
    options: Vec<u8>,
    archive: FileDigest,
}

impl ExtractionCache {
    pub(super) fn new(
        output: &BuildArtifact,
        archive: &ArtifactValue,
        archive_type: ArchiveType,
        strip_prefix: &[String],
        excludes: &[Regex],
    ) -> Option<Self> {
        let mut key = Blake3StrongHasher::new();
        "buck2.extract_archive.v5".hash(&mut key);
        output.get_path().hash(&mut key);
        let mut options = Blake3StrongHasher::new();
        archive_type.name().hash(&mut options);
        strip_prefix.hash(&mut options);
        excludes
            .iter()
            .map(Regex::as_str)
            .collect::<Vec<_>>()
            .hash(&mut options);
        Some(Self {
            key: key.finalize().as_bytes().to_vec(),
            options: options.finalize().as_bytes().to_vec(),
            archive: archive.digest()?.dupe(),
        })
    }

    pub(super) async fn lookup(
        &self,
        ctx: &dyn ActionExecutionCtx,
        output: &BuildArtifact,
    ) -> buck2_error::Result<Option<ArtifactValue>> {
        let Some(store) = ctx.dep_file_store() else {
            return Ok(None);
        };
        for candidate in store.get_digests(&self.key) {
            if candidate.cli_digest != self.options || candidate.directory_digest != self.archive {
                continue;
            }
            let Some(state) = store.get_entry(candidate.id) else {
                continue;
            };
            // The asynchronous writer can replace the row between the probe and the fetch.
            if state.cli_digest != self.options || state.directory_digest != self.archive {
                continue;
            }
            let [stored] = state.outputs.as_slice() else {
                continue;
            };
            if stored.path != output.get_path().path() {
                continue;
            }
            let StoredOutputValue::Directory(fingerprint) = &stored.value else {
                continue;
            };
            let content_hash = ContentBasedPathHash::new(fingerprint.raw_digest().as_bytes())?;
            let path = ctx
                .fs()
                .resolve_build(output.get_path(), Some(&content_hash))?;
            let entries = ctx
                .materializer()
                .get_artifact_entries_for_materialized_paths(vec![path.clone()], false)
                .await?;
            let Some((_, entry)) = entries.into_iter().next().flatten() else {
                continue;
            };
            let value = ArtifactValue::new(entry, None);
            if !value.is_dir() || value.digest() != Some(fingerprint) {
                continue;
            }
            // The materializer trusts tracked contents, but the output root may have been removed.
            if !fs_util::symlink_metadata_if_exists(ctx.fs().fs().resolve(&path))
                .is_ok_and(|metadata| metadata.is_some_and(|metadata| metadata.is_dir()))
            {
                // Publishing equal content would otherwise reuse the stale materializer entry.
                let _lease = ctx.materializer().prepare_outputs(vec![path]).await?;
                continue;
            }
            if ctx
                .materializer()
                .declare_match(vec![(path, value.dupe())])
                .await?
                .is_match()
            {
                store.note_persisted_hit();
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    pub(super) fn store(
        self,
        ctx: &dyn ActionExecutionCtx,
        output: &BuildArtifact,
        value: &ArtifactValue,
    ) -> bool {
        if let Some(store) = ctx.dep_file_store()
            && let Some(fingerprint) = value.digest()
        {
            // The materializer persists the directory tree; the local action cache needs only its fingerprint.
            store.insert(
                self.key,
                Vec::new(),
                StoredDepFileState {
                    cli_digest: self.options,
                    directory_digest: self.archive,
                    local_worker_directory_digest: None,
                    was_produced_locally: true,
                    declared: Vec::new(),
                    outputs: vec![StoredOutput {
                        path: output.get_path().path().to_owned(),
                        value: StoredOutputValue::Directory(fingerprint.dupe()),
                    }],
                },
            );
            return true;
        }
        false
    }
}
