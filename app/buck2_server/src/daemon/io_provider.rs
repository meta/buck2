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

use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::io::IoProvider;
use buck2_common::io::fs::FsIoProvider;
use buck2_common::io::trace::TracingIoProvider;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_core::fs::project::ProjectRoot;

pub async fn create_io_provider(
    fb: fbinit::FacebookInit,
    project_fs: ProjectRoot,
    root_config: &LegacyBuckConfig,
    cas_digest_config: CasDigestConfig,
    trace_io: bool,
    _use_eden_thrift_read: bool,
) -> buck2_error::Result<Arc<dyn IoProvider>> {
    #[cfg(fbcode_build)]
    {
        use buck2_common::legacy_configs::key::BuckconfigKeyRef;
        use buck2_core::rollout_percentage::RolloutPercentage;

        let allow_eden_io_default =
            RolloutPercentage::from_bool(cfg!(any(target_os = "macos", target_os = "windows")));

        let allow_eden_io = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "allow_eden_io",
            })?
            .unwrap_or(allow_eden_io_default)
            .roll();

        if allow_eden_io {
            if let Some(eden) = buck2_eden::io_provider::EdenIoProvider::new(
                fb,
                &project_fs,
                cas_digest_config,
                _use_eden_thrift_read,
            )
            .await?
            {
                return if trace_io {
                    Ok(Arc::new(TracingIoProvider::new(Box::new(eden))))
                } else {
                    Ok(Arc::new(eden))
                };
            }
        }
    }

    let _allow_unused = (fb, root_config);

    #[cfg(fbcode_build)]
    let is_eden = detect_eden::is_eden(project_fs.root().to_path_buf()).unwrap_or(false);
    #[cfg(not(fbcode_build))]
    let is_eden = false;

    if trace_io {
        Ok(Arc::new(TracingIoProvider::new(Box::new(
            FsIoProvider::new(project_fs, cas_digest_config, is_eden),
        ))))
    } else {
        Ok(Arc::new(FsIoProvider::new(
            project_fs,
            cas_digest_config,
            is_eden,
        )))
    }
}
