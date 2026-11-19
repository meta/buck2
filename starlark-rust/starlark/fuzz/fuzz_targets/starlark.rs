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

#![no_main]

use libfuzzer_sys::fuzz_target;
use starlark::environment::Globals;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::AstModule;
use starlark::syntax::Dialect;

fn run_arbitrary_starlark_err(content: &str) -> starlark::Result<String> {
    let ast: AstModule =
        AstModule::parse("hello_world.star", content.to_owned(), &Dialect::Standard)?;
    let globals: Globals = Globals::standard();
    Module::with_temp_heap(|module| {
        let mut eval: Evaluator = Evaluator::new(&module);
        let value = eval.eval_module(ast, &globals)?;
        Ok(format!("{value:?}"))
    })
}

fn run_arbitrary_starlark(content: &str) -> String {
    match run_arbitrary_starlark_err(content) {
        Ok(v) => v,
        Err(e) => {
            const INTERNAL_ERROR: &str = "(internal error)";
            let s = format!("{e:?}");
            // We want to spot internal errors, but not encourage the fuzzer to write internal error in the input.
            // A sufficiently smart fuzzer might outwit us, but hopefully not too quickly.
            if s.contains(INTERNAL_ERROR) && !content.contains(INTERNAL_ERROR) {
                panic!("Internal error as anyhow::Error: {s}")
            }
            s
        }
    }
}

fuzz_target!(|content: &str| {
    let _ignore = run_arbitrary_starlark(content);
});
