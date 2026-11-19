/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_action_metadata_proto::RemoteDepFile;
use buck2_core::buck2_env;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use remote_execution::TActionResult2;
use remote_execution::TCode;
use tracing::info;

use crate::digest_config::DigestConfig;
use crate::execute::action_digest_and_blobs::ActionDigestAndBlobs;
use crate::execute::dep_file_digest::DepFileDigest;
use crate::execute::result::CommandExecutionResult;
use crate::execute::target::CommandExecutionTarget;
use crate::materialize::materializer::Materializer;
use crate::re::error::RemoteExecutionError;

pub struct CacheUploadInfo<'a> {
    pub target: &'a dyn CommandExecutionTarget,
    pub digest_config: DigestConfig,
    pub mergebase: &'a Option<String>,
    pub re_platform: &'a remote_execution::Platform,
    /// Whether locally produced results may be published. Results that came from RE
    /// (remote executions and action-cache hits) are already published.
    pub allow_cache_upload: bool,
}

#[async_trait]
pub trait IntoRemoteDepFile: Send {
    fn remote_dep_file_action(
        &self,
        digest_config: DigestConfig,
        mergebase: &Option<String>,
        re_platform: &remote_execution::Platform,
    ) -> ActionDigestAndBlobs;

    async fn make_remote_dep_file(
        &mut self,
        digest_config: DigestConfig,
        fs: &ArtifactFs,
        materializer: &dyn Materializer,
        re_use_case: RemoteExecutorUseCase,
        result: &CommandExecutionResult,
    ) -> buck2_error::Result<Option<RemoteDepFile>>;
}

pub struct CacheUploadResults {
    pub cache_upload_outcome: CacheUploadOutcome,
    pub dep_file_cache_upload_outcome: DepFileCacheUploadOutcome,
    pub dep_file_cache_upload_key: Option<DepFileDigest>,
}

pub enum DepFileCacheUploadOutcome {
    NoDepFileBundle,
    UnsupportedExecutionKind,
    LocalUploadNotAllowed,
    LocalInPlaceOfRemote,
    Attempted(CacheUploadOutcome),
}

impl DepFileCacheUploadOutcome {
    pub fn attempted(outcome: CacheUploadOutcome) -> Self {
        Self::Attempted(outcome)
    }

    pub fn outcome(&self) -> Option<&CacheUploadOutcome> {
        match self {
            Self::NoDepFileBundle
            | Self::UnsupportedExecutionKind
            | Self::LocalUploadNotAllowed
            | Self::LocalInPlaceOfRemote => None,
            Self::Attempted(outcome) => Some(outcome),
        }
    }

    pub fn uploaded(&self) -> bool {
        match self {
            Self::NoDepFileBundle
            | Self::UnsupportedExecutionKind
            | Self::LocalUploadNotAllowed
            | Self::LocalInPlaceOfRemote => false,
            Self::Attempted(outcome) => outcome.uploaded(),
        }
    }

    pub fn to_proto(&self) -> buck2_data::UploadResult {
        match self {
            Self::NoDepFileBundle | Self::UnsupportedExecutionKind => {
                buck2_data::UploadResult::NotAttempted
            }
            Self::LocalUploadNotAllowed => buck2_data::UploadResult::ActionUploadNotAllowed,
            Self::LocalInPlaceOfRemote => buck2_data::UploadResult::LocalInPlaceOfRemote,
            Self::Attempted(outcome) => outcome.to_proto(),
        }
    }
}

pub enum CacheUploadOutcome {
    Success,
    RejectedOutputExceedsLimit { max_bytes: u64 },
    RejectedPermissionDenied { reason: String },
    RejectedSymlinkOutput,
    FailedUploadActionBlobs { error: buck2_error::Error },
    FailedUploadOutputs { error: buck2_error::Error },
    FailedWriteActionResult { error: buck2_error::Error },
    FailedOther { error: buck2_error::Error },
    ExecutorUploadDisabled,
    HadDepFileBundle,
    NonLocalExecution,
    DepFileEntryExists,
    ActionUploadNotAllowed,
    LocalInPlaceOfRemote,
}

impl CacheUploadOutcome {
    pub fn did_cache_upload(&self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn uploaded(&self) -> bool {
        self.did_cache_upload()
    }

    pub fn to_proto(&self) -> buck2_data::UploadResult {
        match self {
            Self::Success => buck2_data::UploadResult::Uploaded,
            Self::RejectedOutputExceedsLimit { .. } => {
                buck2_data::UploadResult::RejectedOutputExceedsLimit
            }
            Self::RejectedPermissionDenied { .. } => {
                buck2_data::UploadResult::RejectedPermissionDenied
            }
            Self::RejectedSymlinkOutput { .. } => buck2_data::UploadResult::RejectedSymlinkOutput,
            Self::FailedUploadActionBlobs { .. } => {
                buck2_data::UploadResult::FailedUploadActionBlobs
            }
            Self::FailedUploadOutputs { .. } => buck2_data::UploadResult::FailedUploadOutputs,
            Self::FailedWriteActionResult { .. } => {
                buck2_data::UploadResult::FailedWriteActionResult
            }
            Self::FailedOther { .. } => buck2_data::UploadResult::FailedOther,
            Self::ExecutorUploadDisabled { .. } => buck2_data::UploadResult::ExecutorUploadDisabled,
            Self::HadDepFileBundle => buck2_data::UploadResult::HadDepFileBundle,
            Self::DepFileEntryExists => buck2_data::UploadResult::DepFileEntryExists,
            Self::ActionUploadNotAllowed => buck2_data::UploadResult::ActionUploadNotAllowed,
            Self::NonLocalExecution => buck2_data::UploadResult::NonLocalExecution,
            Self::LocalInPlaceOfRemote => buck2_data::UploadResult::LocalInPlaceOfRemote,
        }
    }

    pub fn error(&self) -> String {
        match self {
            Self::Success
            | Self::ExecutorUploadDisabled { .. }
            | Self::HadDepFileBundle
            | Self::DepFileEntryExists
            | Self::ActionUploadNotAllowed
            | Self::NonLocalExecution
            | Self::LocalInPlaceOfRemote => String::new(),
            Self::RejectedOutputExceedsLimit { max_bytes, .. } => {
                format!("Rejected: OutputExceedsLimit({max_bytes})")
            }
            Self::RejectedPermissionDenied { reason, .. } => {
                format!("Rejected: PermissionDenied (permission check error: {reason})")
            }
            Self::RejectedSymlinkOutput { .. } => "Rejected: SymlinkOutput".to_owned(),
            Self::FailedUploadActionBlobs { error, .. }
            | Self::FailedUploadOutputs { error, .. }
            | Self::FailedWriteActionResult { error, .. }
            | Self::FailedOther { error, .. } => format!("{:#}", error),
        }
    }

    pub fn re_error_code(&self) -> Option<String> {
        match self {
            Self::Success
            | Self::ExecutorUploadDisabled { .. }
            | Self::HadDepFileBundle
            | Self::DepFileEntryExists
            | Self::ActionUploadNotAllowed
            | Self::NonLocalExecution
            | Self::LocalInPlaceOfRemote => None,
            Self::RejectedOutputExceedsLimit { .. } | Self::RejectedSymlinkOutput { .. } => None,
            Self::RejectedPermissionDenied { .. } => Some(TCode::PERMISSION_DENIED.to_string()),
            Self::FailedUploadActionBlobs { error, .. }
            | Self::FailedUploadOutputs { error, .. }
            | Self::FailedWriteActionResult { error, .. }
            | Self::FailedOther { error, .. } => {
                match error.find_typed_context::<RemoteExecutionError>() {
                    Some(e) => Some(e.code.to_string()),
                    _ => Some("OTHER_ERRORS".to_owned()),
                }
            }
        }
    }

    pub fn log_and_create_result(
        self,
        digest_str: &str,
        error_on_cache_upload: bool,
    ) -> buck2_error::Result<Self> {
        match &self {
            Self::Success => {
                info!("Cache upload for `{}` succeeded", digest_str);
            }
            Self::RejectedOutputExceedsLimit { .. }
            | Self::RejectedPermissionDenied { .. }
            | Self::RejectedSymlinkOutput { .. } => {
                info!(
                    "Cache upload for `{}` rejected: {}",
                    digest_str,
                    self.error()
                );
            }
            Self::FailedUploadActionBlobs { .. }
            | Self::FailedUploadOutputs { .. }
            | Self::FailedWriteActionResult { .. }
            | Self::FailedOther { .. } => {
                info!("Cache upload for `{}` failed: {}", digest_str, self.error());
            }
            Self::ExecutorUploadDisabled { .. }
            | Self::HadDepFileBundle
            | Self::DepFileEntryExists
            | Self::ActionUploadNotAllowed
            | Self::NonLocalExecution
            | Self::LocalInPlaceOfRemote => {
                info!("Cache upload for `{}` not attempted", digest_str);
            }
        };
        if !self.uploaded() && error_on_cache_upload {
            Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::CacheUploadFailed,
                "cache_upload_failed"
            ))
        } else {
            Ok(self)
        }
    }
}

// This is for quick testing of cache upload without configuring executors.
pub fn force_cache_upload() -> buck2_error::Result<bool> {
    buck2_env!(
        "BUCK2_TEST_FORCE_CACHE_UPLOAD",
        bool,
        applicability = testing
    )
}

/// A single purpose trait to handle cache uploads
#[async_trait]
pub trait UploadCache: Send + Sync {
    /// Given information about the command and its result, upload the result
    /// and related items to the cache.
    /// Return information about why the main action upload did or did not occur.
    async fn upload(
        &self,
        info: &CacheUploadInfo<'_>,
        execution_result: &CommandExecutionResult,
        re_result: Option<&mut TActionResult2>,
        dep_file_bundle: Option<&mut dyn IntoRemoteDepFile>,
        action_digest_and_blobs: &ActionDigestAndBlobs,
    ) -> buck2_error::Result<CacheUploadResults>;
}

/// A no-op cache uploader for when cache uploading is disabled
pub struct NoOpCacheUploader {}

#[async_trait]
impl UploadCache for NoOpCacheUploader {
    async fn upload(
        &self,
        _info: &CacheUploadInfo<'_>,
        _execution_result: &CommandExecutionResult,
        _re_result: Option<&mut TActionResult2>,
        _dep_file_bundle: Option<&mut dyn IntoRemoteDepFile>,
        _action_digest_and_blobs: &ActionDigestAndBlobs,
    ) -> buck2_error::Result<CacheUploadResults> {
        Ok(CacheUploadResults {
            cache_upload_outcome: CacheUploadOutcome::ExecutorUploadDisabled,
            dep_file_cache_upload_outcome: DepFileCacheUploadOutcome::NoDepFileBundle,
            dep_file_cache_upload_key: None,
        })
    }
}
