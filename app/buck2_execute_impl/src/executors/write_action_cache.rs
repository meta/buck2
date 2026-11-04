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

use async_trait::async_trait;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::execution_types::executor_config::RePlatformFields;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_execute::digest::CasDigestToReExt;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::execute::action_digest_and_blobs::ActionDigestAndBlobs;
use buck2_execute::execute::blobs::ActionBlobs;
use buck2_execute::execute::request::ActionMetadataBlobData;
use buck2_execute::execute::write_action_cache::PopulateOutcome;
use buck2_execute::execute::write_action_cache::WriteActionCache;
use buck2_execute::re::client::ActionCacheWriteType;
use buck2_execute::re::manager::ManagedRemoteExecutionClient;
use buck2_execute::re::ttl::re_expiration_from_ttl;
use dupe::Dupe;
use jiff::Timestamp;
use remote_execution as RE;
use remote_execution::TActionResult2;

use crate::executors::action_cache_upload_permission_checker::ActionCacheUploadPermissionChecker;
use crate::executors::to_re_platform::RePlatformFieldsToRePlatform;

/// The one output's name in an entry. Two writes with the same content share an entry whatever
/// their output paths, so the name says nothing about either.
const OUTPUT_NAME: &str = "content";

/// The write action cache of a remote-enabled executor configuration.
pub struct ReWriteActionCache {
    pub re_client: ManagedRemoteExecutionClient,
    pub platform: RePlatformFields,
    /// Whether the executor configuration publishes local results, as the cache uploader is
    /// given to run actions only when it does.
    pub populates: bool,
    pub cache_upload_permission_checker: Arc<ActionCacheUploadPermissionChecker>,
}

impl ReWriteActionCache {
    /// Uploads `content` with `blobs`, whichever the CAS lacks, as an action's inputs are
    /// uploaded: behind the CAS semaphore, with the presence check in front.
    async fn upload_blobs(
        &self,
        mut blobs: ActionBlobs,
        content: &[u8],
        digest: &TrackedFileDigest,
    ) -> buck2_error::Result<()> {
        blobs.add_blob(digest.dupe(), ActionMetadataBlobData(content.to_vec()));
        self.re_client
            .upload_blobs(&blobs)
            .await
            .buck_error_context("Uploading a write's content")?;
        Ok(())
    }
}

#[async_trait]
impl WriteActionCache for ReWriteActionCache {
    fn use_case(&self) -> RemoteExecutorUseCase {
        self.re_client.use_case
    }

    fn populates(&self) -> bool {
        self.populates
    }

    async fn lookup(
        &self,
        action: &ActionDigestAndBlobs,
    ) -> buck2_error::Result<Option<TActionResult2>> {
        let response = self
            .re_client
            .action_cache(action.action, &self.platform.to_re_platform(), None)
            .await?;
        Ok(response.map(|response| response.action_result))
    }

    async fn upload(
        &self,
        content: &[u8],
        digest: &TrackedFileDigest,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<()> {
        self.upload_blobs(ActionBlobs::new(digest_config), content, digest)
            .await
    }

    async fn populate(
        &self,
        action: &ActionDigestAndBlobs,
        content: &[u8],
        digest: &TrackedFileDigest,
        is_executable: bool,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<PopulateOutcome> {
        if !self.populates {
            return Err(internal_error!(
                "A write was recorded under an executor that publishes no local results"
            ));
        }
        if let Err(reason) = self
            .cache_upload_permission_checker
            .has_permission_to_upload_to_cache(&self.re_client, &self.platform, digest_config)
            .await?
        {
            return Ok(PopulateOutcome::PermissionDenied { reason });
        }

        // The action cache reads an entry's action back from the CAS before accepting the
        // entry, so the action's blobs go up with the content.
        self.upload_blobs(action.blobs.clone(), content, digest)
            .await?;

        let mut result = TActionResult2 {
            output_files: vec![RE::TFile {
                digest: RE::DigestWithStatus {
                    digest: digest.to_re(),
                    ..Default::default()
                },
                name: OUTPUT_NAME.to_owned(),
                existed: false,
                executable: is_executable,
                ttl: 0,
                ..Default::default()
            }],
            exit_code: 0,
            ..Default::default()
        };
        let response = self
            .re_client
            .write_action_result(
                action.action,
                &mut result,
                None,
                &self.platform.to_re_platform(),
                ActionCacheWriteType::WriteAction,
            )
            .await
            .buck_error_context("Recording a write's content in the action cache")?;
        // The entry does not outlive its outputs, so its TTL is a lower bound on the content's.
        if response.ttl_seconds > 0 {
            digest.update_expires(re_expiration_from_ttl(
                Timestamp::now(),
                response.ttl_seconds,
                &action.action,
            ));
        }
        Ok(PopulateOutcome::Populated)
    }
}
