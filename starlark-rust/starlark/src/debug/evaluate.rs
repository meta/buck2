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

use crate::collections::SmallMap;
use crate::debug::inspect::to_scope_names_by_local_slot_id;
use crate::eval::Evaluator;
use crate::eval::runtime::slots::LocalSlotIdCapturedOrNot;
use crate::syntax::AstModule;
use crate::values::StringValue;
use crate::values::Value;

impl<'v> Evaluator<'v, '_, '_> {
    /// Evaluate statements in the existing context. This function is designed for debugging,
    /// not production use.
    ///
    /// There are lots of health warnings on this code. Might not work with frozen modules, unassigned variables,
    /// nested definitions etc. It would be a bad idea to rely on the results of continued execution
    /// after evaluating stuff randomly.
    pub fn eval_statements(&mut self, statements: AstModule) -> crate::Result<Value<'v>> {
        // We are doing a lot of funky stuff here. It's amazing anything works, so let's not push our luck with GC.
        self.disable_gc();

        // Everything must be evaluated with the current heap (or we'll lose memory), which means
        // the current module (eval.module_env).
        // We also want access to the module variables (fine), the locals (need to move them over),
        // and the frozen variables (move them over).
        // Afterwards, we want to put everything back - locals can move back to locals, modules
        // can stay where they are, but frozen values are discarded.

        // We want all the local variables to be available to the module, so we capture
        // everything before, shove the local variables into the module, and then revert after
        let original_module: SmallMap<StringValue<'v>, Option<Value<'v>>> = self
            .module_env
            .mutable_names()
            .all_names_and_slots()
            .into_iter()
            .map(|(name, slot)| (name, self.module_env.slots().get_slot(slot)))
            .collect();

        // Push all the frozen variables into the module
        if let Some(frozen) = self.top_frame_def_frozen_module(true)? {
            let frozen = &frozen.as_ref().value;
            for (name, slot) in frozen.names.symbols() {
                if let Some(value) = frozen.get_slot(slot) {
                    self.module_env.set(&name, value)
                }
            }
        }

        // Push all local variables into the module
        let locals = self
            .call_stack
            .to_function_values()
            .into_iter()
            .rev()
            .find_map(to_scope_names_by_local_slot_id);
        if let Some(names) = &locals {
            for (slot, name) in names.iter().enumerate() {
                if let Some(value) = self
                    .current_frame
                    .get_slot_slow(LocalSlotIdCapturedOrNot(slot as u32))
                {
                    self.module_env.set(name.as_str(), value)
                }
            }
        }

        let globals = self.top_frame_def_info_for_debugger()?.value.globals;
        let res = self.eval_module(statements, &globals);

        // Now put the Module back how it was before we started, as best we can
        // and move things into locals if that makes sense
        if let Some(names) = &locals {
            for (slot, name) in names.iter().enumerate() {
                if let Some(value) = self.module_env.get(name.as_str()) {
                    self.current_frame
                        .set_slot_slow(LocalSlotIdCapturedOrNot(slot as u32), value)
                }
            }
            for (name, slot) in self.module_env.mutable_names().all_names_and_slots() {
                match original_module.get(&name) {
                    None => {
                        self.module_env.mutable_names().hide_name(&name);
                        self.module_env.slots().unset_slot(slot);
                    }
                    Some(Some(value)) => self.module_env.slots().set_slot(slot, *value),
                    Some(None) => self.module_env.slots().unset_slot(slot),
                }
            }
        }

        res
    }
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use itertools::Itertools;
    use starlark_derive::starlark_module;
    use starlark_syntax::error::StarlarkResultExt;

    use super::*;
    use crate as starlark;
    use crate::assert;
    use crate::environment::GlobalsBuilder;
    use crate::syntax::Dialect;
    use crate::values::NoSerialize;
    use crate::values::ProvidesStaticType;
    use crate::values::StarlarkPagablePanic;
    use crate::values::StarlarkValue;
    use crate::values::Trace;
    use crate::values::starlark_value;
    use crate::wasm::is_wasm;

    #[derive(
        ProvidesStaticType,
        Trace,
        Allocative,
        Debug,
        NoSerialize,
        StarlarkPagablePanic,
        derive_more::Display
    )]
    #[display("unfreezable")]
    struct Unfreezable;

    #[starlark_value(type = "unfreezable")]
    impl<'v> StarlarkValue<'v> for Unfreezable {}

    #[starlark_module]
    fn debugger(builder: &mut GlobalsBuilder) {
        fn debug_evaluate<'v>(
            code: &str,
            eval: &mut Evaluator<'v, '_, '_>,
        ) -> anyhow::Result<Value<'v>> {
            let ast =
                AstModule::parse("interactive", code.to_owned(), &Dialect::AllOptionsInternal)
                    .into_anyhow_result()?;
            eval.eval_statements(ast).into_anyhow_result()
        }

        fn no_freeze<'v>(eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
            Ok(eval.heap().alloc_complex_no_freeze(Unfreezable))
        }
    }

    fn debugger_assert() -> assert::Assert<'static> {
        let mut a = assert::Assert::new();
        a.disable_static_typechecking();
        a.globals_add(debugger);
        a
    }

    #[test]
    fn test_debug_evaluate() {
        if is_wasm() {
            return;
        }

        let mut a = debugger_assert();
        let check = r#"
assert_eq(debug_evaluate("1+2"), 3)
x = 10
assert_eq(debug_evaluate("x"), 10)
assert_eq(debug_evaluate("x = 5"), None)
assert_eq(x, 5)
y = [20]
debug_evaluate("y.append(30)")
assert_eq(y, [20, 30])
"#;
        // Check evaluation works at the root
        a.pass(check);
        // And inside functions
        a.pass(&format!(
            "def local():\n{}\nlocal()",
            check.lines().map(|x| format!("    {x}")).join("\n")
        ));

        // Check we get the right stack frames
        a.pass(
            r#"
def foo(x, y, z):
    return bar(y)
def bar(x):
    return debug_evaluate("x")
assert_eq(foo(1, 2, 3), 2)
"#,
        );

        // Check we can access module-level and globals
        a.pass(
            r#"
x = 7
def bar(y):
    return debug_evaluate("x + y")
assert_eq(bar(4), 4 + 7)
"#,
        );

        // Check module-level access works in imported modules
        a.module(
            "test",
            r#"
x = 7
z = 2
def bar(y):
    assert_eq(x, 7)
    debug_evaluate("x = 20")
    assert_eq(x, 7) # doesn't work for frozen variables
    return debug_evaluate("x + y + z")
"#,
        );
        a.pass("load('test', 'bar'); assert_eq(bar(4), 4 + 7 + 2)");
    }

    /// The debugger exposes a function local as a module variable, then hides the name. The value
    /// must not stay behind in the module's slot, or freezing the module fails.
    #[test]
    fn test_debug_evaluate_does_not_leak_non_freezable_local() {
        if is_wasm() {
            return;
        }

        let a = debugger_assert();
        a.pass_module(
            r#"
def _inner():
    local_x = no_freeze()
    debug_evaluate("local_x")
_inner()
"#,
        );
    }

    /// The unreachable branch makes the compiler reserve the module name `future` with an
    /// unassigned slot. Cleanup must put that slot back to unassigned.
    #[test]
    fn test_debug_evaluate_restores_unassigned_module_slot() {
        if is_wasm() {
            return;
        }

        let a = debugger_assert();
        a.pass_module(
            r#"
if False:
    future = None
def _inner():
    future = no_freeze()
    debug_evaluate("future")
_inner()
"#,
        );
    }
}
