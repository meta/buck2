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

use starlark::codemap::CodeMap;
use starlark::codemap::Pos;
use starlark::codemap::ResolvedSpan;
use starlark::codemap::Span;
use starlark::syntax::AstModule;
use starlark_syntax::syntax::ast::ArgumentP;
use starlark_syntax::syntax::ast::AssignP;
use starlark_syntax::syntax::ast::AstArgumentP;
use starlark_syntax::syntax::ast::AstExprP;
use starlark_syntax::syntax::ast::AstLiteral;
use starlark_syntax::syntax::ast::AstNoPayload;
use starlark_syntax::syntax::ast::AstStmtP;
use starlark_syntax::syntax::ast::ExprP;
use starlark_syntax::syntax::ast::LoadArgP;
use starlark_syntax::syntax::ast::ParameterP;
use starlark_syntax::syntax::ast::StmtP;
use starlark_syntax::syntax::module::AstModuleFields;
use starlark_syntax::syntax::uniplate::Visit;

use crate::definition::utf16_column_to_byte_offset;

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum AutocompleteType {
    /// Offer completions of all available symbol. Cursor is e.g. at the start of a line,
    /// or in the right hand side of an assignment.
    Default,
    /// Offer completions of loadable modules. Cursor is in the module path of a load statement.
    LoadPath {
        current_value: String,
        current_span: ResolvedSpan,
    },
    /// Offer completions of symbols in a loaded module. Cursor is in a load statement, but
    /// after the module path.
    LoadSymbol {
        path: String,
        current_span: ResolvedSpan,
        previously_loaded: Vec<String>,
    },
    /// Offer completions of target names. Cursor is in a literal string, but not in a load statement
    /// or a visibility-like declaration.
    String {
        current_value: String,
        current_span: ResolvedSpan,
    },
    /// Offer completions of function parameters names. Cursor is in a function call. ALSO offer
    /// regular symbol completions, since this might be e.g. a positional argument, in which cases
    /// parameter names don't matter/help the user.
    Parameter {
        function_name: String,
        function_name_span: ResolvedSpan,
        /// Those parameters that have already been used in this function call
        previously_used_named_parameters: Vec<String>,
    },
    /// Offer completions of type names.
    Type,
    /// Don't offer any completions. Cursor is e.g. in a comment.
    None,
}

pub(crate) trait AstModuleInspect {
    /// Walks through the AST to find the type of the expression at the given position.
    /// Based on that, returns an enum that can be used to determine what kind of
    /// autocomplete should be performed. For example, path in a `load` statement versus
    /// a variable name.
    fn get_auto_complete_type(&self, line: u32, col: u32) -> Option<AutocompleteType>;
}

impl AstModuleInspect for AstModule {
    fn get_auto_complete_type(&self, line: u32, col: u32) -> Option<AutocompleteType> {
        let line_span = match self.codemap().line_span_opt(line as usize) {
            None => {
                // The document got edited to add new lines, just bail out
                return None;
            }
            Some(line_span) => line_span,
        };
        let line_text = self.codemap().source_span(line_span);
        let current_pos = line_span.begin() + utf16_column_to_byte_offset(line_text, col) as u32;

        // Walk through the AST to find a node matching the current position.
        fn walk_and_find_completion_type(
            codemap: &CodeMap,
            position: Pos,
            stmt: Visit<AstNoPayload>,
        ) -> Option<AutocompleteType> {
            // Utility function to get the span of a string literal without the quotes.
            fn string_span_without_quotes(codemap: &CodeMap, span: Span) -> ResolvedSpan {
                let mut span = codemap.resolve_span(span);
                span.begin.column += 1;
                span.end.column -= 1;
                span
            }

            let span = match &stmt {
                Visit::Stmt(stmt) => stmt.span,
                Visit::Expr(expr) => expr.span,
            };
            let contains_pos = span.contains(position);
            if !contains_pos {
                return None;
            }

            match &stmt {
                Visit::Stmt(AstStmtP {
                    node: StmtP::Assign(AssignP { lhs, ty, rhs }),
                    ..
                }) => {
                    if lhs.span.contains(position) {
                        return Some(AutocompleteType::None);
                    }
                    if let Some(type_) = ty {
                        if type_.span.contains(position) {
                            return Some(AutocompleteType::Type);
                        }
                    }
                    if rhs.span.contains(position) {
                        return walk_and_find_completion_type(codemap, position, Visit::Expr(rhs));
                    }
                }
                Visit::Stmt(AstStmtP {
                    node: StmtP::AssignModify(dest, _, expr),
                    ..
                }) => {
                    if dest.span.contains(position) {
                        return Some(AutocompleteType::None);
                    } else if expr.span.contains(position) {
                        return walk_and_find_completion_type(codemap, position, Visit::Expr(expr));
                    }
                }
                Visit::Stmt(AstStmtP {
                    node: StmtP::Load(load),
                    ..
                }) => {
                    if load.module.span.contains(position) {
                        return Some(AutocompleteType::LoadPath {
                            current_value: load.module.to_string(),
                            current_span: string_span_without_quotes(codemap, load.module.span),
                        });
                    }

                    for LoadArgP { local, .. } in &load.args {
                        if local.span.contains(position) {
                            return Some(AutocompleteType::LoadSymbol {
                                path: load.module.to_string(),
                                current_span: string_span_without_quotes(codemap, local.span),
                                previously_loaded: load
                                    .args
                                    .iter()
                                    .filter(|load_arg| &load_arg.local != local)
                                    .map(|LoadArgP { local, .. }| local.to_string())
                                    .collect(),
                            });
                        }
                    }

                    return Some(AutocompleteType::None);
                }
                Visit::Stmt(AstStmtP {
                    node: StmtP::Def(def),
                    ..
                }) => {
                    // If the cursor is in the name of the function, don't offer any completions.
                    if def.name.span.contains(position) {
                        return Some(AutocompleteType::None);
                    }
                    // If the cursor is in one of the arguments, only offer completions for
                    // default values for the arguments.
                    for arg in def.params.iter() {
                        if !arg.span.contains(position) {
                            continue;
                        }
                        match &arg.node {
                            ParameterP::Normal(_, Some(type_), None) => {
                                if type_.span.contains(position) {
                                    return Some(AutocompleteType::Type);
                                }
                            }
                            ParameterP::Normal(_, type_, Some(expr)) => {
                                if let Some(type_) = type_ {
                                    if type_.span.contains(position) {
                                        return Some(AutocompleteType::Type);
                                    }
                                }
                                if expr.span.contains(position) {
                                    return walk_and_find_completion_type(
                                        codemap,
                                        position,
                                        Visit::Expr(expr),
                                    );
                                }
                            }
                            _ => {}
                        }

                        return Some(AutocompleteType::None);
                    }
                    if let Some(return_type) = &def.return_type {
                        if return_type.span.contains(position) {
                            return Some(AutocompleteType::Type);
                        }
                    }

                    return walk_and_find_completion_type(
                        codemap,
                        position,
                        Visit::Stmt(&def.body),
                    );
                }
                Visit::Expr(AstExprP {
                    node: ExprP::Call(name, args),
                    span,
                }) => {
                    if name.span.contains(position) {
                        return Some(AutocompleteType::Default);
                    }
                    let get_previously_used_argument_names = || {
                        args.args
                            .iter()
                            .filter_map(|arg| match arg {
                                AstArgumentP {
                                    node: ArgumentP::Named(name, _),
                                    ..
                                } => Some(name.node.clone()),
                                _ => None,
                            })
                            .collect()
                    };
                    for arg in &args.args {
                        if !arg.span.contains(position) {
                            continue;
                        }
                        match &arg.node {
                            ArgumentP::Named(arg_name, value) => {
                                if arg_name.span.contains(position) {
                                    return Some(AutocompleteType::Parameter {
                                        function_name: name.to_string(),
                                        function_name_span: codemap.resolve_span(name.span),
                                        previously_used_named_parameters:
                                            get_previously_used_argument_names(),
                                    });
                                } else if value.span.contains(position) {
                                    return walk_and_find_completion_type(
                                        codemap,
                                        position,
                                        Visit::Expr(value),
                                    );
                                }
                            }
                            ArgumentP::Positional(expr) => {
                                return match expr {
                                    AstExprP {
                                        node: ExprP::Identifier(_),
                                        ..
                                    } => {
                                        // Typing a literal, might be meant as a parameter name.
                                        Some(AutocompleteType::Parameter {
                                            function_name: name.to_string(),
                                            function_name_span: codemap.resolve_span(name.span),
                                            previously_used_named_parameters:
                                                get_previously_used_argument_names(),
                                        })
                                    }
                                    _ => walk_and_find_completion_type(
                                        codemap,
                                        position,
                                        Visit::Expr(expr),
                                    ),
                                };
                            }
                            ArgumentP::Args(expr) | ArgumentP::KwArgs(expr) => {
                                return walk_and_find_completion_type(
                                    codemap,
                                    position,
                                    Visit::Expr(expr),
                                );
                            }
                        }
                    }
                    // No matches? We might be in between empty braces (new function call),
                    // e.g. `foo(|)`. However, we don't want to offer completions for
                    // when the cursor is at the very end of the function call, e.g. `foo()|`.
                    return Some(if span.end() != position {
                        AutocompleteType::Parameter {
                            function_name: name.to_string(),
                            function_name_span: codemap.resolve_span(name.span),
                            previously_used_named_parameters: get_previously_used_argument_names(),
                        }
                    } else {
                        // Don't offer completions right after the function call.
                        AutocompleteType::None
                    });
                }
                Visit::Expr(AstExprP {
                    node: ExprP::Literal(AstLiteral::String(str)),
                    ..
                }) => {
                    return Some(AutocompleteType::String {
                        current_value: str.to_string(),
                        current_span: string_span_without_quotes(codemap, span),
                    });
                }
                Visit::Stmt(stmt) => {
                    let mut result = None;
                    stmt.visit_children(|stmt| {
                        if let Some(r) = walk_and_find_completion_type(codemap, position, stmt) {
                            result = Some(r);
                        }
                    });
                    return result;
                }
                Visit::Expr(expr) => {
                    let mut result = None;
                    expr.visit_expr(|expr| {
                        if let Some(r) =
                            walk_and_find_completion_type(codemap, position, Visit::Expr(expr))
                        {
                            result = Some(r);
                        }
                    });
                    return result;
                }
            }

            None
        }

        walk_and_find_completion_type(self.codemap(), current_pos, Visit::Stmt(self.statement()))
            .or(Some(AutocompleteType::Default))
    }
}
