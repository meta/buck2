/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::cmp;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use buck2_common::starlark_profiler::StarlarkProfileDataAndStatsDyn;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use starlark::eval::ProfileData;

use crate::dice::starlark_provider::StarlarkEvalKind;

#[derive(Debug, Clone, Allocative)]
pub struct StarlarkProfileDataAndStats {
    #[allocative(skip)] // OK to skip because used only when profiling enabled.
    pub profile_data: ProfileData,
    pub targets: Vec<StarlarkEvalKind>,
    pub(crate) initialized_at: Instant,
    pub(crate) finalized_at: Instant,
    pub(crate) total_retained_bytes: usize,
}

impl StarlarkProfileDataAndStatsDyn for StarlarkProfileDataAndStats {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl StarlarkProfileDataAndStats {
    pub fn duration(&self) -> Duration {
        self.finalized_at.duration_since(self.initialized_at)
    }

    pub fn total_retained_bytes(&self) -> usize {
        self.total_retained_bytes
    }

    pub fn merge<'a>(
        datas: impl IntoIterator<Item = &'a StarlarkProfileDataAndStats>,
    ) -> buck2_error::Result<StarlarkProfileDataAndStats> {
        let datas = Vec::from_iter(datas);
        let mut iter = datas.iter().copied();
        let first = iter
            .next()
            .internal_error("empty collection of profile data")?;
        let mut total_retained_bytes = first.total_retained_bytes;
        let mut initialized_at = first.initialized_at;
        let mut finalized_at = first.finalized_at;

        for data in iter {
            initialized_at = cmp::min(initialized_at, data.initialized_at);
            finalized_at = cmp::max(finalized_at, data.finalized_at);
            total_retained_bytes += data.total_retained_bytes;
        }

        let profile_data = ProfileData::merge(datas.iter().map(|data| &data.profile_data))?;

        Ok(StarlarkProfileDataAndStats {
            profile_data,
            targets: datas
                .iter()
                .flat_map(|data| data.targets.iter().cloned())
                .collect(),
            initialized_at,
            finalized_at,
            total_retained_bytes,
        })
    }

    pub fn downcast(
        profile_data: &dyn StarlarkProfileDataAndStatsDyn,
    ) -> buck2_error::Result<&Self> {
        profile_data.as_any().downcast_ref::<Self>().ok_or_else(|| {
            internal_error!("There's only one implementation of StarlarkProfileDataAndStatsDyn")
        })
    }
}
