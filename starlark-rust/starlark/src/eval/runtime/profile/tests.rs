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

#![cfg(test)]

use starlark_syntax::golden_test_template::golden_test_template;
use starlark_syntax::syntax::AstModule;
use starlark_syntax::syntax::Dialect;

use crate::assert::test_functions;
use crate::environment::GlobalsBuilder;
use crate::environment::Module;
use crate::eval::Evaluator;
use crate::eval::ProfileData;
use crate::eval::ProfileMode;
use crate::eval::runtime::profile::data::ProfileDataImpl;
use crate::values::layout::heap::name::StarlarkTestHeapName;

fn test_profile_golden_for_mode(mode: ProfileMode) {
    Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);
        eval.enable_profile(&mode).unwrap();
        eval.eval_module(
            AstModule::parse(
                "test.star",
                r#"
def inner(x: int):
    if noop():
        return 10
    else:
        for x in range(10):
            noop()

def test():
    r = []
    for x in noop([1, 2, 3, 4, 5]):
        inner(x)
        r += noop([1] * 3)
    return r

test()
test()
test()

R = test()
"#
                .to_owned(),
                &Dialect::AllOptionsInternal,
            )
            .unwrap(),
            &GlobalsBuilder::extended().with(test_functions).build(),
        )
        .unwrap();

        let mut profile_data = match mode {
            ProfileMode::HeapSummaryRetained | ProfileMode::HeapFlameRetained => {
                drop(eval);
                let module = module
                    .freeze_named(StarlarkTestHeapName::frozen_heap_name())
                    .unwrap();
                module.heap_profile().unwrap()
            }
            _ => eval.gen_profile().unwrap(),
        };

        if let ProfileDataImpl::HeapRetained(profile)
        | ProfileDataImpl::HeapAllocated(profile)
        | ProfileDataImpl::HeapFlameRetained(profile)
        | ProfileDataImpl::HeapFlameAllocated(profile)
        | ProfileDataImpl::HeapSummaryRetained(profile)
        | ProfileDataImpl::HeapSummaryAllocated(profile) = &mut profile_data.profile
        {
            profile.normalize_for_golden_tests();
        }

        if let Some(profile) = profile_data.gen_flame_data().unwrap() {
            golden_test_template(
                &format!(
                    "src/eval/runtime/profile/golden/{}.flame.golden",
                    mode.name().replace('-', "_")
                ),
                &profile,
            );
        }

        match mode {
            ProfileMode::HeapFlameRetained
            | ProfileMode::HeapFlameAllocated
            | ProfileMode::TimeFlame => {}
            _ => {
                golden_test_template(
                    &format!(
                        "src/eval/runtime/profile/golden/{}.csv.golden",
                        mode.name().replace('-', "_")
                    ),
                    &profile_data.gen_csv().unwrap(),
                );
            }
        }
        // Smoke test for profile merging.
        ProfileData::merge([&profile_data, &profile_data]).unwrap();
        crate::Result::Ok(())
    })
    .unwrap();
}

#[test]
fn test_profile_golden_heap_allocated() {
    test_profile_golden_for_mode(ProfileMode::HeapAllocated);
}

#[test]
fn test_profile_golden_heap_retained() {
    test_profile_golden_for_mode(ProfileMode::HeapAllocated);
}

#[test]
fn test_profile_golden_heap_summary_allocated() {
    test_profile_golden_for_mode(ProfileMode::HeapSummaryAllocated);
}

#[test]
fn test_profile_golden_heap_summary_retained() {
    test_profile_golden_for_mode(ProfileMode::HeapSummaryRetained);
}

#[test]
fn test_profile_golden_heap_flame_allocated() {
    test_profile_golden_for_mode(ProfileMode::HeapFlameAllocated);
}

#[test]
fn test_profile_golden_heap_flame_retained() {
    test_profile_golden_for_mode(ProfileMode::HeapFlameRetained);
}

#[test]
fn test_profile_golden_statement() {
    test_profile_golden_for_mode(ProfileMode::Statement);
}

#[test]
fn test_profile_golden_coverage() {
    test_profile_golden_for_mode(ProfileMode::Coverage);
}

#[test]
fn test_profile_golden_bytecode() {
    test_profile_golden_for_mode(ProfileMode::Bytecode);
}

#[test]
fn test_profile_golden_bytecode_pairs() {
    test_profile_golden_for_mode(ProfileMode::BytecodePairs);
}

#[test]
fn test_profile_golden_time_flame() {
    test_profile_golden_for_mode(ProfileMode::TimeFlame);
}

#[test]
fn test_profile_golden_typecheck() {
    test_profile_golden_for_mode(ProfileMode::Typecheck);
}
