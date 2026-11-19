/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use crate::eval::runtime::profile::bc::BcPairsProfileData;
use crate::eval::runtime::profile::bc::BcPairsProfilerType;
use crate::eval::runtime::profile::bc::BcProfileData;
use crate::eval::runtime::profile::bc::BcProfilerType;
use crate::eval::runtime::profile::flamegraph::FlameGraphData;
use crate::eval::runtime::profile::heap::HeapAllocatedProfilerType;
use crate::eval::runtime::profile::heap::HeapFlameAllocatedProfilerType;
use crate::eval::runtime::profile::heap::HeapFlameRetainedProfilerType;
use crate::eval::runtime::profile::heap::HeapRetainedProfilerType;
use crate::eval::runtime::profile::heap::HeapSummaryAllocatedProfilerType;
use crate::eval::runtime::profile::heap::HeapSummaryRetainedProfilerType;
use crate::eval::runtime::profile::mode::ProfileMode;
use crate::eval::runtime::profile::profiler_type::ProfilerType;
use crate::eval::runtime::profile::stmt::CoverageProfileType;
use crate::eval::runtime::profile::stmt::StmtProfileData;
use crate::eval::runtime::profile::stmt::StmtProfilerType;
use crate::eval::runtime::profile::time_flame::TimeFlameProfilerType;
use crate::eval::runtime::profile::typecheck::TypecheckProfileData;
use crate::eval::runtime::profile::typecheck::TypecheckProfilerType;
use crate::values::layout::heap::profile::aggregated::AggregateHeapProfileInfo;

#[derive(Debug, thiserror::Error)]
enum ProfileDataError {
    #[error("Empty profile list cannot be merged")]
    EmptyProfileList,
    #[error("Different profile modes in profile")]
    DifferentProfileModes,
}

#[derive(Clone, Debug)]
pub(crate) enum ProfileDataImpl {
    Bc(Box<BcProfileData>),
    BcPairs(BcPairsProfileData),
    HeapRetained(Box<AggregateHeapProfileInfo>),
    HeapAllocated(Box<AggregateHeapProfileInfo>),
    HeapFlameRetained(Box<AggregateHeapProfileInfo>),
    HeapFlameAllocated(Box<AggregateHeapProfileInfo>),
    HeapSummaryRetained(Box<AggregateHeapProfileInfo>),
    HeapSummaryAllocated(Box<AggregateHeapProfileInfo>),
    /// Flame graph data is in milliseconds.
    TimeFlameProfile(FlameGraphData),
    Statement(StmtProfileData),
    Coverage(StmtProfileData),
    Typecheck(TypecheckProfileData),
    None,
}

impl ProfileDataImpl {
    pub(crate) fn profile_mode(&self) -> ProfileMode {
        match self {
            ProfileDataImpl::Bc(_) => ProfileMode::Bytecode,
            ProfileDataImpl::BcPairs(_) => ProfileMode::BytecodePairs,
            ProfileDataImpl::HeapRetained(_) => ProfileMode::HeapRetained,
            ProfileDataImpl::HeapAllocated(_) => ProfileMode::HeapAllocated,
            ProfileDataImpl::HeapFlameRetained(_) => ProfileMode::HeapFlameRetained,
            ProfileDataImpl::HeapFlameAllocated(_) => ProfileMode::HeapFlameAllocated,
            ProfileDataImpl::HeapSummaryRetained(_) => ProfileMode::HeapSummaryRetained,
            ProfileDataImpl::HeapSummaryAllocated(_) => ProfileMode::HeapSummaryAllocated,
            ProfileDataImpl::TimeFlameProfile(_) => ProfileMode::TimeFlame,
            ProfileDataImpl::Statement(_) => ProfileMode::Statement,
            ProfileDataImpl::Coverage(_) => ProfileMode::Coverage,
            ProfileDataImpl::Typecheck(_) => ProfileMode::Typecheck,
            ProfileDataImpl::None => ProfileMode::None,
        }
    }
}

/// Collected profiling data.
#[derive(Clone, Debug)]
pub struct ProfileData {
    pub(crate) profile: ProfileDataImpl,
}

fn _assert_profile_data_send_sync() {
    fn _assert_send_sync<T: Send + Sync>() {}
    _assert_send_sync::<ProfileData>();
}

impl ProfileData {
    /// Profile mode used to collect this data.
    pub fn profile_mode(&self) -> ProfileMode {
        self.profile.profile_mode()
    }

    /// Generate a string with flamegraph profile data, depending on profile type.
    pub fn gen_flame_data(&self) -> crate::Result<Option<String>> {
        match &self.profile {
            ProfileDataImpl::TimeFlameProfile(profile) => Ok(Some(profile.write())),
            ProfileDataImpl::HeapRetained(profile)
            | ProfileDataImpl::HeapAllocated(profile)
            | ProfileDataImpl::HeapFlameRetained(profile)
            | ProfileDataImpl::HeapFlameAllocated(profile) => {
                Ok(Some(profile.gen_flame_graph_data()))
            }
            _ => Ok(None),
        }
    }

    /// Generate a string with csv profile data, depending on profile type.
    pub fn gen_csv(&self) -> crate::Result<String> {
        match &self.profile {
            ProfileDataImpl::Bc(bc) => Ok(bc.gen_csv()),
            ProfileDataImpl::BcPairs(bc_pairs) => Ok(bc_pairs.gen_csv()),
            ProfileDataImpl::HeapRetained(profile) | ProfileDataImpl::HeapAllocated(profile) => {
                Ok(profile.gen_summary_csv())
            }
            ProfileDataImpl::HeapSummaryRetained(profile)
            | ProfileDataImpl::HeapSummaryAllocated(profile) => Ok(profile.gen_summary_csv()),
            ProfileDataImpl::TimeFlameProfile(data) => Ok(data.write()),
            ProfileDataImpl::Statement(data) => Ok(data.write_to_string()),
            ProfileDataImpl::Coverage(data) => Ok(data.write_coverage()),
            ProfileDataImpl::Typecheck(data) => Ok(data.gen_csv()),
            _ => Ok("".to_owned()),
        }
    }

    /// Merge profiles (aggregate).
    pub fn merge<'a>(
        profiles: impl IntoIterator<Item = &'a ProfileData>,
    ) -> crate::Result<ProfileData> {
        let profiles = Vec::from_iter(profiles);

        if let [one] = profiles.as_slice() {
            // If there's only one profile, just return it instead of invoking merge.
            // - Merge may fail
            // - Or may not be implemented for the profile type
            return Ok(ProfileData::clone(one));
        }

        let profile_mode = match profiles.first() {
            None => return Err(crate::Error::new_other(ProfileDataError::EmptyProfileList)),
            Some(p) => p.profile.profile_mode(),
        };
        for p in &profiles {
            if p.profile.profile_mode() != profile_mode {
                return Err(crate::Error::new_other(
                    ProfileDataError::DifferentProfileModes,
                ));
            }
        }
        let profile = match &profile_mode {
            ProfileMode::Bytecode => BcProfilerType::merge_profiles(&profiles)?.profile,
            ProfileMode::BytecodePairs => BcPairsProfilerType::merge_profiles(&profiles)?.profile,
            ProfileMode::HeapAllocated => {
                HeapAllocatedProfilerType::merge_profiles(&profiles)?.profile
            }
            ProfileMode::HeapRetained => {
                HeapRetainedProfilerType::merge_profiles(&profiles)?.profile
            }
            ProfileMode::HeapSummaryAllocated => {
                HeapSummaryAllocatedProfilerType::merge_profiles(&profiles)?.profile
            }
            ProfileMode::HeapSummaryRetained => {
                HeapSummaryRetainedProfilerType::merge_profiles(&profiles)?.profile
            }
            ProfileMode::HeapFlameAllocated => {
                HeapFlameAllocatedProfilerType::merge_profiles(&profiles)?.profile
            }
            ProfileMode::HeapFlameRetained => {
                HeapFlameRetainedProfilerType::merge_profiles(&profiles)?.profile
            }
            ProfileMode::TimeFlame => TimeFlameProfilerType::merge_profiles(&profiles)?.profile,
            ProfileMode::Typecheck => TypecheckProfilerType::merge_profiles(&profiles)?.profile,
            ProfileMode::Statement => StmtProfilerType::merge_profiles(&profiles)?.profile,
            ProfileMode::Coverage => CoverageProfileType::merge_profiles(&profiles)?.profile,
            ProfileMode::None => ProfileDataImpl::None,
        };
        Ok(ProfileData { profile })
    }
}
