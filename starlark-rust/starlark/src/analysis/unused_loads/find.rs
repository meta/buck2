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

use std::collections::HashMap;

use dupe::Dupe;
use starlark_syntax::codemap::CodeMap;
use starlark_syntax::codemap::FileSpanRef;
use starlark_syntax::codemap::Span;
use starlark_syntax::codemap::Spanned;
use starlark_syntax::slice_vec_ext::SliceExt;
use starlark_syntax::syntax::ast::LoadArgP;
use starlark_syntax::syntax::ast::LoadP;
use starlark_syntax::syntax::ast::StmtP;
use starlark_syntax::syntax::module::AstModuleFields;
use starlark_syntax::syntax::top_level_stmts::top_level_stmts;

use crate::environment::names::MutableNames;
use crate::eval::compiler::scope::BindingId;
use crate::eval::compiler::scope::ModuleScopes;
use crate::eval::compiler::scope::ResolvedIdent;
use crate::eval::compiler::scope::Slot;
use crate::eval::compiler::scope::payload::CstPayload;
use crate::eval::compiler::scope::scope_resolver_globals::ScopeResolverGlobals;
use crate::syntax::AstModule;
use crate::syntax::Dialect;
use crate::values::FrozenHeap;
use crate::values::HeapEdge;
use crate::values::any::StarlarkAny;

/// Unused load statement.
pub(crate) struct UnusedLoad {
    /// Span of the whole statement.
    pub(crate) span: Span,
    /// Number of names the statement loads.
    arg_count: usize,
    /// Unused local names, e. g. `x` in `load("foo", x="y")`.
    pub(crate) unused_args: Vec<UnusedLoadArg>,
}

/// Unused name in a `load` statement.
pub(crate) struct UnusedLoadArg {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) span: Span,
    /// The argument together with its trailing comma, if any.
    pub(crate) span_with_trailing_comma: Span,
}

impl UnusedLoad {
    /// If the whole `load` statement is unused.
    pub(crate) fn all_unused(&self) -> bool {
        self.unused_args.len() == self.arg_count
    }
}

/// Check if there are `@unused` markers on the lines with the given span.
fn has_unused_marker_in_range(span: FileSpanRef) -> bool {
    let begin_line = span.file.find_line(span.span.begin());
    let end_line = span.file.find_line(span.span.end());
    for line_no in begin_line..=end_line {
        let line = span.file.source_line(line_no);
        if line.ends_with("@unused") || line.contains("@unused ") {
            return true;
        }
    }
    false
}

/// Parse the module and find unused loads.
pub(crate) fn find_unused_loads(
    name: &str,
    program: &str,
) -> crate::Result<(CodeMap, Vec<UnusedLoad>)> {
    let module = AstModule::parse(name, program.to_owned(), &Dialect::AllOptionsInternal)?;
    FrozenHeap::temp(|heap| {
        let names = MutableNames::new();
        let (codemap, statement, dialect, ..) = module.into_parts();
        let codemap = heap.alloc_simple_typed(StarlarkAny::new(codemap));
        let module_scopes = ModuleScopes::check_module_err(
            &names,
            heap,
            HeapEdge::identity(),
            &HashMap::new(),
            statement,
            ScopeResolverGlobals::unknown(),
            codemap,
            &dialect,
        )?;

        let mut loads = Vec::new();

        struct LoadSymbol<'a, 'f> {
            arg: &'a LoadArgP<CstPayload<'f>>,
            binding_id: BindingId,
            used: bool,
        }

        struct LoadWip<'a, 'f> {
            load: Spanned<&'a LoadP<CstPayload<'f>>>,
            args: Vec<LoadSymbol<'a, 'f>>,
        }

        impl LoadWip<'_, '_> {
            fn any_unused(&self) -> bool {
                self.args.iter().any(|arg| !arg.used)
            }
        }

        for top in top_level_stmts(&module_scopes.cst) {
            if let StmtP::Load(load) = &**top {
                let args = load.args.try_map(|arg| {
                    anyhow::Ok(LoadSymbol {
                        arg,
                        binding_id: arg
                            .local
                            .payload
                            .ok_or_else(|| anyhow::anyhow!("payload is not set"))?,
                        used: false,
                    })
                })?;
                loads.push(LoadWip {
                    load: Spanned {
                        span: top.span,
                        node: load,
                    },
                    args,
                });
            }
        }

        for top in top_level_stmts(&module_scopes.cst) {
            top.visit_ident(|ident| {
                let ResolvedIdent::Slot(Slot::Module(_), binding_id) = ident
                    .payload
                    .ok_or_else(|| anyhow::anyhow!("ident is not resolved (internal error)"))?
                else {
                    return Ok(());
                };
                for load in &mut loads {
                    for arg in &mut load.args {
                        if arg.binding_id == binding_id {
                            arg.used = true;
                        }
                    }
                }
                anyhow::Ok(())
            })?;
        }

        let mut unused = Vec::new();

        for load in loads {
            if !load.any_unused() {
                continue;
            }
            let unused_args: Vec<_> = load
                .args
                .into_iter()
                .filter_map(|arg| {
                    if arg.used {
                        None
                    } else if has_unused_marker_in_range(FileSpanRef {
                        file: &codemap,
                        span: arg.arg.span_with_trailing_comma(),
                    }) {
                        None
                    } else {
                        Some(UnusedLoadArg {
                            span: arg.arg.span(),
                            span_with_trailing_comma: arg.arg.span_with_trailing_comma(),
                        })
                    }
                })
                .collect();
            if unused_args.is_empty() {
                continue;
            }
            unused.push(UnusedLoad {
                span: load.load.span,
                arg_count: load.load.node.args.len(),
                unused_args,
            });
        }

        Ok(((*codemap).dupe(), unused))
    })
}
