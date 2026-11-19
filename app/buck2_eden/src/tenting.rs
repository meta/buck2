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
use buck2_common::tenting::TentingAclProvider;
use buck2_common::tenting::TentingStatus;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::soft_error;
use buck2_error::buck2_error;
use edenfs::FileAttributes;
use edenfs::GetAttributesFromFilesParams;
use edenfs::SyncBehavior;
use fbinit::FacebookInit;
use tokio::sync::Semaphore;

use crate::connection::EdenConnectionManager;
use crate::error::EdenDataIntoResult;

pub struct EdenTentingAclProvider {
    manager: EdenConnectionManager,
}

impl EdenTentingAclProvider {
    pub fn try_new(
        fb: FacebookInit,
        project_root: &ProjectRoot,
        semaphore: Option<Semaphore>,
    ) -> buck2_error::Result<Option<Self>> {
        let manager = match EdenConnectionManager::new(fb, project_root, semaphore)? {
            Some(manager) => manager,
            None => return Ok(None),
        };
        Ok(Some(Self { manager }))
    }
}

fn no_sync() -> SyncBehavior {
    SyncBehavior {
        syncTimeoutSeconds: Some(0),
        ..Default::default()
    }
}

#[async_trait]
impl TentingAclProvider for EdenTentingAclProvider {
    async fn get_tenting_acl_names(
        &self,
        path: &ProjectRelativePath,
    ) -> buck2_error::Result<TentingStatus> {
        let requested_attributes = i64::from(i32::from(FileAttributes::ACLs));

        let params = GetAttributesFromFilesParams {
            mountPoint: self.manager.get_mount_point(),
            paths: self.manager.project_path_list_as_eden_path_list([path]),
            requestedAttributes: requested_attributes,
            sync: no_sync(),
            ..Default::default()
        };

        // Best-effort: any failure below returns `Unknown` (never blocks the build, and is
        // distinct from `NotTented` so a transient failure can't clear tenting metadata).
        // Unexpected failures go to `soft_error!`; the expected "ACLs attribute not deployed
        // yet" case is logged at `debug!` to avoid rollout spam (see the `aclInfo` arm).
        let attrs = match self
            .manager
            .with_eden(|eden| eden.getAttributesFromFilesV2(&params))
            .await
        {
            Ok(attrs) => attrs,
            Err(e) => {
                soft_error!(
                    "tenting_acl_eden_call_failed",
                    buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "get_tenting_acl_names({}): EdenFS call failed: {:#}",
                        path,
                        e
                    )
                );
                return Ok(TentingStatus::Unknown);
            }
        };

        let data = match attrs.res.into_iter().next() {
            Some(entry) => match entry.into_result() {
                Ok(data) => data,
                Err(e) => {
                    soft_error!(
                        "tenting_acl_attribute_error",
                        buck2_error!(
                            buck2_error::ErrorTag::Input,
                            "get_tenting_acl_names({}): attribute error: {:#}",
                            path,
                            e
                        )
                    );
                    return Ok(TentingStatus::Unknown);
                }
            },
            None => {
                soft_error!(
                    "tenting_acl_empty_response",
                    buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "get_tenting_acl_names({}): empty response from EdenFS",
                        path
                    )
                );
                return Ok(TentingStatus::Unknown);
            }
        };

        match data.aclInfo {
            Some(edenfs::AclInfoOrError::aclInfo(info)) => {
                if !info.underAcl {
                    // Lookup succeeded and the path is under no ACL.
                    return Ok(TentingStatus::NotTented);
                }
                let acls: Vec<String> = info
                    .acls
                    .into_iter()
                    .map(|entry| entry.repoRegionAcl)
                    .collect();
                if acls.is_empty() {
                    // Under an ACL but no resolvable names: we cannot emit meaningful
                    // tenting metadata and must not report "not tented", so treat this as
                    // undetermined (fail-safe: preserves any existing tenting state).
                    soft_error!(
                        "tenting_acl_under_acl_no_names",
                        buck2_error!(
                            buck2_error::ErrorTag::Input,
                            "get_tenting_acl_names({}): underAcl=true but no ACL names returned",
                            path
                        )
                    );
                    return Ok(TentingStatus::Unknown);
                }
                Ok(TentingStatus::Tented(Arc::from(acls)))
            }
            Some(other) => {
                tracing::debug!(
                    "get_tenting_acl_names({}): aclInfo error: {:?}",
                    path,
                    other
                );
                Ok(TentingStatus::Unknown)
            }
            None => Ok(TentingStatus::Unknown),
        }
    }
}
