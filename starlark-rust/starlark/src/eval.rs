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

//! Evaluate some code, typically done by creating an [`Evaluator`], then calling
//! [`eval_module`](Evaluator::eval_module).

pub(crate) mod bc;
pub(crate) mod compiler;
mod params;
pub(crate) mod runtime;
pub(crate) mod soft_error;

use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

use dupe::Dupe;
pub use runtime::arguments::Arguments;
pub use runtime::before_stmt::BeforeStmtFunc;
pub use runtime::before_stmt::BeforeStmtFuncDyn;
pub use runtime::evaluator::Evaluator;
pub use runtime::file_loader::FileLoader;
pub use runtime::file_loader::ReturnFileLoader;
pub use runtime::params::parser::ParametersParser;
pub use runtime::params::spec::ParametersSpec;
pub use runtime::params::spec::ParametersSpecParam;
pub use runtime::profile::data::ProfileData;
pub use runtime::profile::mode::ProfileMode;
pub use soft_error::SoftErrorHandler;
pub use soft_error::set_global_soft_error_handler;
pub use starlark_syntax::call_stack::CallStack;
use starlark_syntax::slice_vec_ext::SliceExt;
use starlark_syntax::syntax::module::AstModule;
use starlark_syntax::syntax::module::AstModuleFields;

use crate::codemap::CodeMap;
use crate::collections::symbol::symbol::Symbol;
use crate::docs::DocString;
use crate::environment::Globals;
use crate::eval::compiler::Compiler;
use crate::eval::compiler::def::DefInfo;
use crate::eval::compiler::scope::ModuleScopes;
use crate::eval::compiler::scope::ScopeId;
use crate::eval::compiler::scope::scope_resolver_globals::ScopeResolverGlobals;
pub use crate::eval::params::param_specs;
use crate::eval::runtime::arguments::ArgNames;
use crate::eval::runtime::arguments::ArgumentsFull;
use crate::eval::runtime::evaluator;
use crate::register_starlark_any;
use crate::syntax::DialectTypes;
use crate::values::FrozenValueTyped;
use crate::values::StringValue;
use crate::values::Value;
use crate::values::any::StarlarkAny;
use crate::values::any_complex::StarlarkAnyComplex;

// Register CodeMap for use with StarlarkAny
register_starlark_any!(CodeMap);

impl<'v, 'a, 'e> Evaluator<'v, 'a, 'e> {
    /// Evaluate an [`AstModule`] with this [`Evaluator`], modifying the in-scope
    /// [`Module`](crate::environment::Module) as appropriate.
    pub fn eval_module(&mut self, ast: AstModule, globals: &Globals) -> crate::Result<Value<'v>> {
        #[cfg(not(target_arch = "wasm32"))]
        let start = Instant::now();

        let (codemap, statement, dialect, typecheck) = ast.into_parts();

        // Compilation and execution happen within one scope on the module's frozen heap; the
        // compiler's products live there.
        let module_env = self.module_env;
        let res = module_env.heaps().frozen_heap(|fh, edge, seal_edge| {
            let codemap = fh.alloc_simple_typed(StarlarkAny::new(codemap.dupe()));

            if let Some(docstring) = DocString::extract_raw_starlark_docstring(&statement) {
                module_env.set_docstring(docstring)
            }

            let ModuleScopes {
                cst,
                module_slot_count,
                scope_data,
                top_level_stmt_count,
            } = globals.data().by_ref_with_reconstructor(|globals, r| {
                // The edge makes the module's frozen heap keep the globals alive.
                let gedge = r.frozen_edge(fh);
                ModuleScopes::check_module_err(
                    module_env.mutable_names(),
                    fh,
                    edge,
                    &HashMap::new(),
                    statement,
                    ScopeResolverGlobals {
                        globals: Some((globals, gedge)),
                    },
                    codemap,
                    &dialect,
                )
            })?;

            let globals = fh.alloc_simple_typed(StarlarkAny::new(globals.dupe()));

            let scope_names = scope_data.get_scope(ScopeId::module());
            let local_names: Box<[StringValue]> = scope_names.used.clone().into_boxed_slice();
            let parent = scope_names.parent.clone().into_boxed_slice();

            module_env.slots().ensure_slots(module_slot_count);
            // Like the bytecode, the module's `DefInfo` crosses to the evaluator's heap once.
            let module_def_info = fh.alloc_simple_typed(StarlarkAnyComplex::new(
                DefInfo::for_module(codemap, local_names.clone(), parent, globals),
            ));
            let module_def_info =
                FrozenValueTyped::from_typed(module_def_info).expect("allocated in a frozen heap");
            let old_def_info = self.module_def_info.replace(edge.rebrand(module_def_info));

            self.call_stack.alloc_if_needed(
                self.max_callstack_size
                    .unwrap_or(evaluator::DEFAULT_STACK_SIZE),
            )?;

            // Set up the world to allow evaluation (do NOT use ? from now on)

            self.call_stack.push(Value::new_none(), None).unwrap();

            // Evaluation
            let mut compiler = Compiler {
                scope_data,
                locals: Vec::new(),
                globals,
                codemap,
                eval: self,
                fh,
                edge,
                seal_edge,
                check_types: dialect.enable_types == DialectTypes::Enable,
                top_level_stmt_count,
                typecheck,
                local_as_values: Vec::new(),
            };

            let res = compiler.eval_module(cst, &local_names);

            // Clean up the world, putting everything back
            self.call_stack.pop();

            self.module_def_info = old_def_info;

            crate::Result::Ok(res)
        })?;

        #[cfg(not(target_arch = "wasm32"))]
        self.module_env.add_eval_duration(start.elapsed());

        self.run_infrequent_instr_checks()?;

        // Return the result of evaluation
        res.map_err(|e| e.into_error())
    }

    /// Evaluate a function stored in a [`Value`], passing in `positional` and `named` arguments.
    pub fn eval_function(
        &mut self,
        function: Value<'v>,
        positional: &[Value<'v>],
        named: &[(&str, Value<'v>)],
    ) -> crate::Result<Value<'v>> {
        let names = named.map(|(s, _)| (Symbol::new(s), self.heap().alloc(*s)));
        let named = named.map(|x| x.1);
        let params = Arguments(ArgumentsFull {
            pos: positional,
            named: &named,
            names: ArgNames::new_check_unique(&names)?,
            args: None,
            kwargs: None,
        });
        self.call_stack.alloc_if_needed(
            self.max_callstack_size
                .unwrap_or(evaluator::DEFAULT_STACK_SIZE),
        )?;
        // eval_module pushes an "empty" call stack frame. other places expect that first frame to be ignorable, and
        // so we push an empty frame too (otherwise things would ignore this function's own frame).
        let res = self.with_call_stack(Value::new_none(), None, |this| {
            function.invoke(&params, this)
        });

        self.run_infrequent_instr_checks()?;

        res
    }
}
