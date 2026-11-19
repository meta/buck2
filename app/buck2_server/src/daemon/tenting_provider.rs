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

use buck2_common::tenting::TentingAclProvider;
use buck2_core::fs::project::ProjectRoot;

pub fn create_tenting_acl_provider(
    _fb: fbinit::FacebookInit,
    _project_root: &ProjectRoot,
) -> Option<Arc<dyn TentingAclProvider>> {
    #[cfg(fbcode_build)]
    {
        match buck2_eden::tenting::EdenTentingAclProvider::try_new(_fb, _project_root, None) {
            Ok(Some(provider)) => Some(Arc::new(provider)),
            Ok(None) => None,
            Err(e) => {
                buck2_core::soft_error_or_panic!(
                    "tenting_acl_provider_creation_failed",
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Failed to create TentingAclProvider: {:#}",
                        e
                    )
                );
                None
            }
        }
    }
    #[cfg(not(fbcode_build))]
    {
        None
    }
}
