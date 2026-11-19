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

use std::fmt::Debug;

use allocative::Allocative;
use dupe::Dupe;

use crate::eval::ProfileMode;
use crate::eval::runtime::profile::data::ProfileData;
use crate::eval::runtime::profile::data::ProfileDataImpl;
use crate::eval::runtime::profile::profiler_type::ProfilerType;
use crate::values::Heap;
use crate::values::Value;
use crate::values::layout::heap::profile::aggregated::AggregateHeapProfileInfo;

pub(crate) struct HeapAllocatedProfilerType;
pub(crate) struct HeapRetainedProfilerType;
pub(crate) struct HeapSummaryAllocatedProfilerType;
pub(crate) struct HeapFlameAllocatedProfilerType;
pub(crate) struct HeapSummaryRetainedProfilerType;
pub(crate) struct HeapFlameRetainedProfilerType;

impl ProfilerType for HeapAllocatedProfilerType {
    type Data = Box<AggregateHeapProfileInfo>;
    const PROFILE_MODE: ProfileMode = ProfileMode::HeapAllocated;

    fn data_from_generic(profile_data: &ProfileDataImpl) -> Option<&Self::Data> {
        match profile_data {
            ProfileDataImpl::HeapAllocated(data) => Some(data),
            _ => None,
        }
    }

    fn data_to_generic(data: Self::Data) -> ProfileDataImpl {
        ProfileDataImpl::HeapAllocated(data)
    }

    fn merge_profiles_impl(profiles: &[&Self::Data]) -> starlark_syntax::Result<Self::Data> {
        Ok(Box::new(AggregateHeapProfileInfo::merge(
            profiles.iter().map(|x| &***x),
        )))
    }
}

impl ProfilerType for HeapSummaryAllocatedProfilerType {
    type Data = Box<AggregateHeapProfileInfo>;
    const PROFILE_MODE: ProfileMode = ProfileMode::HeapSummaryAllocated;

    fn data_from_generic(profile_data: &ProfileDataImpl) -> Option<&Self::Data> {
        match profile_data {
            ProfileDataImpl::HeapSummaryAllocated(data) => Some(data),
            _ => None,
        }
    }

    fn data_to_generic(data: Self::Data) -> ProfileDataImpl {
        ProfileDataImpl::HeapSummaryAllocated(data)
    }

    fn merge_profiles_impl(profiles: &[&Self::Data]) -> starlark_syntax::Result<Self::Data> {
        Ok(Box::new(AggregateHeapProfileInfo::merge(
            profiles.iter().map(|x| &***x),
        )))
    }
}

impl ProfilerType for HeapFlameAllocatedProfilerType {
    type Data = Box<AggregateHeapProfileInfo>;
    const PROFILE_MODE: ProfileMode = ProfileMode::HeapFlameAllocated;

    fn data_from_generic(profile_data: &ProfileDataImpl) -> Option<&Self::Data> {
        match profile_data {
            ProfileDataImpl::HeapFlameAllocated(data) => Some(data),
            _ => None,
        }
    }

    fn data_to_generic(data: Self::Data) -> ProfileDataImpl {
        ProfileDataImpl::HeapFlameAllocated(data)
    }

    fn merge_profiles_impl(profiles: &[&Self::Data]) -> starlark_syntax::Result<Self::Data> {
        Ok(Box::new(AggregateHeapProfileInfo::merge(
            profiles.iter().map(|x| &***x),
        )))
    }
}

impl ProfilerType for HeapRetainedProfilerType {
    type Data = Box<AggregateHeapProfileInfo>;
    const PROFILE_MODE: ProfileMode = ProfileMode::HeapRetained;

    fn data_from_generic(profile_data: &ProfileDataImpl) -> Option<&Self::Data> {
        match profile_data {
            ProfileDataImpl::HeapRetained(data) => Some(data),
            _ => None,
        }
    }

    fn data_to_generic(data: Self::Data) -> ProfileDataImpl {
        ProfileDataImpl::HeapRetained(data)
    }

    fn merge_profiles_impl(profiles: &[&Self::Data]) -> starlark_syntax::Result<Self::Data> {
        Ok(Box::new(AggregateHeapProfileInfo::merge(
            profiles.iter().map(|x| &***x),
        )))
    }
}

impl ProfilerType for HeapSummaryRetainedProfilerType {
    type Data = Box<AggregateHeapProfileInfo>;
    const PROFILE_MODE: ProfileMode = ProfileMode::HeapSummaryRetained;

    fn data_from_generic(profile_data: &ProfileDataImpl) -> Option<&Self::Data> {
        match profile_data {
            ProfileDataImpl::HeapSummaryRetained(data) => Some(data),
            _ => None,
        }
    }

    fn data_to_generic(data: Self::Data) -> ProfileDataImpl {
        ProfileDataImpl::HeapSummaryRetained(data)
    }

    fn merge_profiles_impl(profiles: &[&Self::Data]) -> starlark_syntax::Result<Self::Data> {
        Ok(Box::new(AggregateHeapProfileInfo::merge(
            profiles.iter().map(|x| &***x),
        )))
    }
}

impl ProfilerType for HeapFlameRetainedProfilerType {
    type Data = Box<AggregateHeapProfileInfo>;
    const PROFILE_MODE: ProfileMode = ProfileMode::HeapFlameRetained;

    fn data_from_generic(profile_data: &ProfileDataImpl) -> Option<&Self::Data> {
        match profile_data {
            ProfileDataImpl::HeapFlameRetained(data) => Some(data),
            _ => None,
        }
    }

    fn data_to_generic(data: Self::Data) -> ProfileDataImpl {
        ProfileDataImpl::HeapFlameRetained(data)
    }

    fn merge_profiles_impl(profiles: &[&Self::Data]) -> starlark_syntax::Result<Self::Data> {
        Ok(Box::new(AggregateHeapProfileInfo::merge(
            profiles.iter().map(|x| &***x),
        )))
    }
}

#[derive(Copy, Clone, Dupe, Debug, Allocative)]
pub(crate) enum RetainedHeapProfileMode {
    Flame,
    Summary,
    FlameAndSummary,
}

#[derive(Debug, thiserror::Error)]
enum HeapProfileError {
    #[error("heap profile not enabled")]
    NotEnabled,
}

#[derive(Copy, Clone, Dupe, Debug)]
pub(crate) enum HeapProfileFormat {
    FlameGraph,
    Summary,
    FlameGraphAndSummary,
}

pub(crate) struct HeapProfile {
    enabled: bool,
}

impl HeapProfile {
    pub(crate) fn new() -> Self {
        Self { enabled: false }
    }

    pub(crate) fn enable(&mut self) {
        self.enabled = true;
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn record_call_enter<'v>(&self, function: Value<'v>, heap: Heap<'v>) {
        if self.enabled {
            heap.record_call_enter(function);
        }
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn record_call_exit<'v>(&self, heap: Heap<'v>) {
        if self.enabled {
            heap.record_call_exit();
        }
    }

    // We could expose profile on the Heap, but it's an implementation detail that it works here.
    pub(crate) fn r#gen(
        &self,
        heap: Heap<'_>,
        format: HeapProfileFormat,
    ) -> crate::Result<ProfileData> {
        if !self.enabled {
            return Err(crate::Error::new_other(HeapProfileError::NotEnabled));
        }
        Ok(Self::gen_enabled(heap, format))
    }

    pub(crate) fn gen_enabled(heap: Heap<'_>, format: HeapProfileFormat) -> ProfileData {
        match format {
            HeapProfileFormat::FlameGraphAndSummary => {
                Self::write_flame_and_summarized_heap_profile(heap)
            }
            HeapProfileFormat::Summary => Self::write_summarized_heap_profile(heap),
            HeapProfileFormat::FlameGraph => Self::write_flame_heap_profile(heap),
        }
    }

    fn write_flame_heap_profile(heap: Heap<'_>) -> ProfileData {
        let stacks = AggregateHeapProfileInfo::collect(heap, None);
        ProfileData {
            profile: ProfileDataImpl::HeapFlameAllocated(Box::new(stacks)),
        }
    }

    fn write_summarized_heap_profile(heap: Heap<'_>) -> ProfileData {
        let stacks = AggregateHeapProfileInfo::collect(heap, None);
        ProfileData {
            profile: ProfileDataImpl::HeapSummaryAllocated(Box::new(stacks)),
        }
    }

    fn write_flame_and_summarized_heap_profile(heap: Heap<'_>) -> ProfileData {
        let stacks = AggregateHeapProfileInfo::collect(heap, None);
        ProfileData {
            profile: ProfileDataImpl::HeapAllocated(Box::new(stacks)),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::environment::Globals;
    use crate::environment::Module;
    use crate::eval::Evaluator;
    use crate::eval::runtime::profile::heap::HeapProfile;
    use crate::eval::runtime::profile::mode::ProfileMode;
    use crate::syntax::AstModule;
    use crate::syntax::Dialect;
    use crate::values::Heap;
    use crate::values::Value;

    #[test]
    fn test_profiling() -> crate::Result<()> {
        // We don't test that the profile looks any particular way, but we do test it doesn't crash
        let ast = AstModule::parse(
            "foo.bzl",
            r#"
def f(x):
    return (x * 5) + 3
y = 8 * 9 + 2
f
"#
            .to_owned(),
            &Dialect::AllOptionsInternal,
        )?;
        let globals = Globals::standard();
        Heap::temp(|heap| {
            let module = Module::with_heap(heap);
            let module2 = Module::with_heap(heap);
            let module3 = Module::with_heap(heap);

            let mut eval = Evaluator::new(&module);
            eval.enable_profile(&ProfileMode::HeapSummaryAllocated)
                .unwrap();
            let f = eval.eval_module(ast, &globals)?;

            // first check module profiling works
            HeapProfile::write_summarized_heap_profile(module.heap());
            HeapProfile::write_flame_heap_profile(module.heap());

            // second check function profiling works
            let mut eval = Evaluator::new(&module2);
            eval.enable_profile(&ProfileMode::HeapSummaryAllocated)
                .unwrap();
            eval.eval_function(f, &[Value::testing_new_int(100)], &[])?;

            HeapProfile::write_summarized_heap_profile(module2.heap());
            HeapProfile::write_flame_heap_profile(module2.heap());

            // finally, check a user can add values into the heap before/after
            let mut eval = Evaluator::new(&module3);
            module3.heap().alloc("Thing that goes before");
            eval.enable_profile(&ProfileMode::HeapSummaryAllocated)
                .unwrap();
            eval.eval_function(f, &[Value::testing_new_int(100)], &[])?;

            module3.heap().alloc("Thing that goes after");
            HeapProfile::write_summarized_heap_profile(module3.heap());
            HeapProfile::write_flame_heap_profile(module3.heap());

            Ok(())
        })
    }
}
