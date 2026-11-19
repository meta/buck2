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

use buck2_artifact::actions::key::ActionIndex;
use buck2_artifact::actions::key::ActionKey;
use buck2_artifact::artifact::artifact_type::BoundBuildArtifact;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_core::deferred::dynamic::DynamicLambdaResultsKey;
use buck2_core::deferred::key::DeferredHolderKey;
use dupe::Dupe;

pub struct DynamicActionsOutputArtifactBinder {
    key: DeferredHolderKey,
    index: u32,
}

impl DynamicActionsOutputArtifactBinder {
    pub fn new(key: &DynamicLambdaResultsKey) -> Self {
        DynamicActionsOutputArtifactBinder {
            key: DeferredHolderKey::DynamicLambda(Arc::new(key.dupe())),
            index: 0,
        }
    }

    pub fn bind(&mut self, output: OutputArtifact) -> buck2_error::Result<BoundBuildArtifact> {
        // We create ActionKeys that point directly to the dynamic_lambda's
        // output rather than our own. This saves the resolution of the key from
        // needing to first lookup our result just to get forwarded to the lambda's result.
        //
        // This means that we are creating ActionKeys for the lambda and it needs to offset
        // its key's index to account for this (see ActionRegistry where this is done).
        let bound = output
            .bind(ActionKey::new(
                self.key.dupe(),
                ActionIndex::new(self.index),
            ))?
            .dupe();
        self.index += 1;
        Ok(bound)
    }
}
