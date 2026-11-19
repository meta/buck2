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
use std::sync::Arc;

use buck2_core::async_once_cell::AsyncOnceCell;
use buck2_core::execution_types::executor_config::RePlatformFields;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_error::BuckErrorContext;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::re::client::ActionCacheWriteType;
use buck2_execute::re::error::RemoteExecutionError;
use buck2_execute::re::manager::ManagedRemoteExecutionClient;
use buck2_hash::BuckDashMap;
use dupe::Dupe;
use futures::FutureExt;
use remote_execution::TCode;

use crate::executors::empty_action_result::empty_action_result;
use crate::executors::to_re_platform::RePlatformFieldsToRePlatform;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    re_use_case: RemoteExecutorUseCase,
    platform: RePlatformFields,
}

struct CacheValue {
    has_permission_to_upload_to_cache: AsyncOnceCell<Result<(), String>>,
}

/// Check permission to upload to action cache and cache result.
pub struct ActionCacheUploadPermissionChecker {
    /// Permission check does not depend on RE use case,
    /// but since we use these to upload, it is safer to cache the result by them.
    has_permission_to_upload_to_cache: BuckDashMap<CacheKey, Arc<CacheValue>>,
}

impl ActionCacheUploadPermissionChecker {
    pub fn new() -> ActionCacheUploadPermissionChecker {
        ActionCacheUploadPermissionChecker {
            has_permission_to_upload_to_cache: BuckDashMap::default(),
        }
    }

    async fn do_has_permission_to_upload_to_cache(
        &self,
        re_client: &ManagedRemoteExecutionClient,
        platform: &RePlatformFields,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<Result<(), String>> {
        let (action, mut action_result) = empty_action_result(platform, digest_config)?;

        // This is CAS upload, if it fails, something is very broken.
        re_client
            .upload_files_and_directories(Vec::new(), Vec::new(), action.blobs.to_inlined_blobs())
            .await?;

        // This operation requires permission to write.
        let result = re_client
            .write_action_result(
                action.action,
                &mut action_result,
                &platform.to_re_platform(),
                ActionCacheWriteType::PermissionCheck,
            )
            .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => match e.find_typed_context::<RemoteExecutionError>() {
                Some(e) if e.code == TCode::PERMISSION_DENIED => Ok(Err(e.message.clone())),
                _ => Err(e),
            },
        }
    }

    fn cache_value(
        &self,
        re_use_case: RemoteExecutorUseCase,
        platform: &RePlatformFields,
    ) -> Arc<CacheValue> {
        self.has_permission_to_upload_to_cache
            .entry(CacheKey {
                re_use_case,
                platform: platform.clone(),
            })
            .or_insert_with(|| {
                Arc::new(CacheValue {
                    has_permission_to_upload_to_cache: AsyncOnceCell::new(),
                })
            })
            .dupe()
    }

    pub(crate) async fn has_permission_to_upload_to_cache(
        &self,
        re_client: &ManagedRemoteExecutionClient,
        platform: &RePlatformFields,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<Result<(), String>> {
        let cache_value = self.cache_value(re_client.use_case, platform);
        cache_value
            .has_permission_to_upload_to_cache
            .get_or_try_init(async move {
                // Boxed so that the calling/containing future only needs room for the Box pointer, not the entire future.
                self.do_has_permission_to_upload_to_cache(re_client, platform, digest_config)
                    .boxed()
                    .await
            })
            .await
            .cloned()
            .buck_error_context("Upload for permission check")
    }
}

#[cfg(fbcode_build)] // Relies on fbcode future sizes
buck2_util::size_assert::words_of_async_fn_future!(
    ActionCacheUploadPermissionChecker::has_permission_to_upload_to_cache,
    (_, _, _, _),
    ~77
);
