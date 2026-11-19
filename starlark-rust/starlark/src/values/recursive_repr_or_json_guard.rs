/*
 * Copyright 2018 The Starlark in Rust Authors.
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

//! Detect recursion when doing `repr` or `to_json`.

use std::cell::Cell;

use crate::collections::SmallSet;
use crate::hint::unlikely;
use crate::values::Value;
use crate::values::layout::pointer::RawPointer;

/// Pop the stack on drop.
pub(crate) struct ReprStackGuard;

/// Pop the stack on drop.
pub(crate) struct JsonStackGuard;

impl Drop for ReprStackGuard {
    fn drop(&mut self) {
        REPR_STACK.with(|repr_stack| {
            let mut stack = Cell::take(repr_stack);
            let popped = stack.pop();
            debug_assert!(popped.is_some());
            repr_stack.set(stack);
        })
    }
}

impl Drop for JsonStackGuard {
    fn drop(&mut self) {
        JSON_STACK.with(|json_stack| {
            let mut stack = Cell::take(json_stack);
            let popped = stack.pop();
            debug_assert!(popped.is_some());
            json_stack.set(stack);
        })
    }
}

/// Returned when `repr` is called recursively and a cycle is detected.
pub(crate) struct ReprCycle;

/// Why a value cannot be serialized to JSON.
pub(crate) enum JsonStackError {
    /// The value contains itself.
    Cycle,
    /// Serializing the value would run out of native stack.
    TooDeep,
}

/// Stack we refuse to serialize into, leaving it free for the rest of the program.
///
/// Serializing goes through `erased_serde` into monomorphised `serde_json` frames, which cost
/// 10-30 KiB of native stack per level of nesting, so a value only a couple of hundred deep can
/// exhaust the stack. Without this check that aborts the process; with it we stop while there is
/// still stack left and return an error.
///
/// The reserve has to cover several levels of nesting, since it is only checked between them, plus
/// whatever the caller does with the error afterwards.
const JSON_STACK_RESERVE: usize = 256 * 1024;

thread_local! {
    static REPR_STACK: Cell<SmallSet<RawPointer>> = const { Cell::new(SmallSet::new()) };
}

thread_local! {
    static JSON_STACK: Cell<SmallSet<RawPointer>> = const { Cell::new(SmallSet::new()) };
}

/// Push a value to the stack, return error if it is already on the stack.
pub(crate) fn repr_stack_push(value: Value) -> Result<ReprStackGuard, ReprCycle> {
    REPR_STACK.with(|repr_stack| {
        let mut stack = Cell::take(repr_stack);
        let res = if unlikely(!stack.insert(value.ptr_value())) {
            Err(ReprCycle)
        } else {
            Ok(ReprStackGuard)
        };
        repr_stack.set(stack);
        res
    })
}

/// Push a value to the stack, return error if it is already on the stack, or if serializing it
/// would run out of native stack.
pub(crate) fn json_stack_push(value: Value) -> Result<JsonStackGuard, JsonStackError> {
    // `remaining_stack` returns `None` when the platform cannot tell us, in which case there is
    // nothing to check against and we carry on.
    if unlikely(stacker::remaining_stack().is_some_and(|left| left < JSON_STACK_RESERVE)) {
        return Err(JsonStackError::TooDeep);
    }
    JSON_STACK.with(|json_stack| {
        let mut stack = Cell::take(json_stack);
        let res = if unlikely(!stack.insert(value.ptr_value())) {
            Err(JsonStackError::Cycle)
        } else {
            Ok(JsonStackGuard)
        };
        json_stack.set(stack);
        res
    })
}
