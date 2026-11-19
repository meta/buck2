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
use std::time::Instant;

use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use starlark::environment::FrozenModule;
use starlark::eval::Evaluator;
use starlark::eval::ProfileData;
use starlark::eval::ProfileMode;

use crate::dice::starlark_provider::StarlarkEvalKind;
use crate::starlark_profiler::data::StarlarkProfileDataAndStats;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum StarlarkProfilerError {
    #[error(
        "Retained memory profiling is available only for analysis profile \
        or bxl profile (which freezes the module)"
    )]
    RetainedMemoryNotFrozen,
}

pub struct ProfilerData {
    profile_mode: Option<ProfileMode>,

    initialized_at: Option<Instant>,
    finalized_at: Option<Instant>,
    profile_data: Option<buck2_error::Result<ProfileData>>,
}

impl ProfilerData {
    pub(crate) fn new(profile_mode: Option<ProfileMode>) -> Self {
        ProfilerData {
            profile_mode,
            initialized_at: None,
            finalized_at: None,
            profile_data: None,
        }
    }

    /// Prepare an Evaluator to capture output relevant to this profiler.
    pub(crate) fn initialize(&mut self, eval: &mut Evaluator) -> buck2_error::Result<bool> {
        self.initialized_at = Some(Instant::now());
        if let Some(mode) = &self.profile_mode {
            eval.enable_profile(mode)
                .map_err(|e| internal_error!("{}", e))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Post-analysis, produce the output of this profiler.
    pub(crate) fn evaluation_complete(&mut self, eval: &mut Evaluator) {
        self.finalized_at = Some(Instant::now());
        if let Some(mode) = &self.profile_mode {
            if !mode.requires_frozen_module() {
                self.profile_data = Some(
                    eval.gen_profile()
                        .internal_error("error during profile generation"),
                );
            }
        }
    }

    pub fn finish(
        mut self,
        frozen_module: Option<&FrozenModule>,
        target: StarlarkEvalKind,
    ) -> buck2_error::Result<Option<Arc<StarlarkProfileDataAndStats>>> {
        let mode = match self.profile_mode {
            None => {
                return Ok(None);
            }
            Some(v) => v,
        };

        let total_retained_bytes = match (frozen_module, mode.requires_frozen_module()) {
            (None, true) => {
                return Err(StarlarkProfilerError::RetainedMemoryNotFrozen.into());
            }
            (Some(module), requires_frozen) => {
                if requires_frozen {
                    let profile = module
                        .heap_profile()
                        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?;
                    self.profile_data = Some(Ok(profile));
                }

                module
                    .frozen_heap()
                    .allocated_summary()
                    .total_allocated_bytes()
            }
            (None, false) => 0,
        };

        Ok(Some(Arc::new(StarlarkProfileDataAndStats {
            initialized_at: self.initialized_at.internal_error("did not initialize")?,
            finalized_at: self.finalized_at.internal_error("did not finalize")?,
            total_retained_bytes,
            profile_data: self
                .profile_data
                .internal_error("profile_data not initialized")??,
            targets: vec![target],
        })))
    }
}
