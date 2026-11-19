/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_hash::BuckMutMap;
use dupe::Dupe;
use remote_execution::InlinedBlobWithDigest;

use crate::digest::CasDigestToReExt;
use crate::digest_config::DigestConfig;
use crate::execute::request::ActionMetadataBlobData;
use crate::execute::request::ActionMetadataBlobMessage;

/// Contains small blobs referenced from action messages (does not include any file contents blobs).
#[derive(Clone)]
pub struct ActionBlobs(BuckMutMap<TrackedFileDigest, ActionMetadataBlobData>);

impl ActionBlobs {
    pub fn new(digest_config: DigestConfig) -> Self {
        // We add empty files to the input that don't exist in disk; so add
        // the empty digest to blobs, as going to disk would fail.
        let mut blobs = BuckMutMap::default();
        blobs.insert(
            TrackedFileDigest::empty(digest_config.cas_digest_config()),
            ActionMetadataBlobData(Vec::new()),
        );
        Self(blobs)
    }

    pub fn add_blob(&mut self, digest: TrackedFileDigest, data: ActionMetadataBlobData) {
        self.0.insert(digest, data);
    }

    pub fn add_protobuf_message(
        &mut self,
        m: &impl ActionMetadataBlobMessage,
        digest_config: DigestConfig,
    ) -> TrackedFileDigest {
        let blob = ActionMetadataBlobData::from_message(m);
        let digest = TrackedFileDigest::from_content(&blob.0, digest_config.cas_digest_config());
        self.0.insert(digest.dupe(), blob);
        digest
    }

    pub fn keys(&self) -> impl Iterator<Item = &TrackedFileDigest> {
        self.0.keys()
    }

    pub fn get(&self, digest: &TrackedFileDigest) -> Option<&ActionMetadataBlobData> {
        self.0.get(digest)
    }

    pub fn iter(
        &self,
    ) -> std::collections::hash_map::Iter<'_, TrackedFileDigest, ActionMetadataBlobData> {
        self.0.iter()
    }

    pub fn to_inlined_blobs(&self) -> Vec<InlinedBlobWithDigest> {
        self.0
            .iter()
            .map(|(digest, data)| InlinedBlobWithDigest {
                blob: data.0.clone(),
                digest: digest.to_re(),
                ..Default::default()
            })
            .collect()
    }
}
