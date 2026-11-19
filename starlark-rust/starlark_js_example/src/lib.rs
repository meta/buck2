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

use std::mem;
use std::slice;
use std::str;

use starlark::environment::Globals;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::AstModule;
use starlark::syntax::Dialect;
use starlark::values::Value;

#[unsafe(no_mangle)]
pub extern "C" fn allocation(n: usize) -> *mut u8 {
    mem::ManuallyDrop::new(Vec::with_capacity(n)).as_mut_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn evaluate(s: *const u8) -> *mut u8 {
    unsafe {
        let length = u32::from_le_bytes(*(s as *const [u8; 4])) as usize;
        let input = slice::from_raw_parts(s.offset(4), length);
        let output = evaluate_buffers(input);
        mem::ManuallyDrop::new(output).as_mut_ptr()
    }
}

fn evaluate_buffers(input: &[u8]) -> Vec<u8> {
    let contents = str::from_utf8(input).unwrap();
    let result = evaluate_starlark(contents);
    let success = result.is_ok();
    let message = result.unwrap_or_else(|e| e.into_anyhow().to_string());
    let len = message.len();
    let mut buffer = Vec::with_capacity(len + 8);
    buffer.push(if success { 1 } else { 0 });
    buffer.extend(vec![0; 3]);
    buffer.extend_from_slice(&(len as u32).to_le_bytes());
    buffer.extend_from_slice(message.as_bytes());
    buffer
}

fn evaluate_starlark(content: &str) -> Result<String, starlark::Error> {
    let ast: AstModule =
        AstModule::parse("hello_world.star", content.to_owned(), &Dialect::Standard)?;
    let globals = Globals::standard();
    Module::with_temp_heap(|module| {
        let mut eval: Evaluator = Evaluator::new(&module);
        let res: Value = eval.eval_module(ast, &globals)?;
        Ok::<_, starlark::Error>(res.to_string())
    })
}
