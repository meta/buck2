/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Shared utilities for working with load() statements in Starlark/Buck files.
//!
//! Provides common types and functions for parsing and manipulating load statements,
//! used by both `unused_loads` and `sort_loads` autofixes.

use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_text_size::Ranged;
use ruff_text_size::TextRange;

/// Represents a symbol loaded from a load() statement.
#[derive(Debug, Clone, Copy)]
pub struct LoadSymbol<'a> {
    /// The name bound in local scope (LHS of alias or the string value itself).
    pub bound_name: &'a str,
    /// The original symbol name from the module, if different from bound_name (keyword args only).
    pub original_name: Option<&'a str>,
    /// The full text range of the argument (for @unused comment detection).
    pub range: TextRange,
}

/// Represents a load() statement with its parsed components.
#[derive(Debug)]
pub struct LoadStatement<'a> {
    /// Range of the entire statement (for removal).
    pub stmt_range: TextRange,
    /// The module path (first argument).
    pub module_path: &'a str,
    /// Range of the module path argument.
    pub module_path_range: TextRange,
    /// Symbols being loaded (remaining arguments).
    pub symbols: Vec<LoadSymbol<'a>>,
}

/// Check if an expression is a `load` function call.
pub fn is_load_call(expr: &Expr) -> bool {
    if let Expr::Call(call) = expr {
        if let Expr::Name(name) = call.func.as_ref() {
            return name.id.as_str() == "load";
        }
    }
    false
}

/// Extract load statements and their symbols from the module.
pub fn extract_load_statements<'a>(stmts: &'a [Stmt]) -> Vec<LoadStatement<'a>> {
    let mut loads = Vec::new();

    for stmt in stmts.iter() {
        let Stmt::Expr(expr_stmt) = stmt else {
            continue;
        };

        let Expr::Call(call) = expr_stmt.value.as_ref() else {
            continue;
        };

        if !is_load_call(&expr_stmt.value) {
            continue;
        }

        // First positional arg is the module path
        let Some(module_arg) = call.arguments.args.first() else {
            continue;
        };

        let Expr::StringLiteral(module_str) = module_arg else {
            continue;
        };

        let module_path = module_str.value.to_str();

        // Remaining args are symbols
        let mut symbols = Vec::new();

        // Process positional args (skip first which is module path)
        for arg in call.arguments.args.iter().skip(1) {
            if let Expr::StringLiteral(s) = arg {
                symbols.push(LoadSymbol {
                    bound_name: s.value.to_str(),
                    original_name: None,
                    range: arg.range(),
                });
            }
        }

        // Process keyword args: `alias = "original"`
        for kw in call.arguments.keywords.iter() {
            if let (Some(alias), Expr::StringLiteral(s)) = (&kw.arg, &kw.value) {
                symbols.push(LoadSymbol {
                    bound_name: alias.as_str(),
                    original_name: Some(s.value.to_str()),
                    range: kw.range(),
                });
            }
        }

        loads.push(LoadStatement {
            stmt_range: stmt.range(),
            module_path,
            module_path_range: module_arg.range(),
            symbols,
        });
    }

    loads
}

/// Reconstruct a load statement with only the specified symbols.
pub fn reconstruct_load(module_path: &str, symbols: &[LoadSymbol<'_>]) -> String {
    if symbols.is_empty() {
        return String::new();
    }

    let mut parts = vec![format!("\"{}\"", module_path)];
    for sym in symbols {
        if let Some(original_name) = &sym.original_name {
            // Keyword: alias = "original"
            parts.push(format!("{}=\"{}\"", sym.bound_name, original_name));
        } else {
            // Positional: "name"
            parts.push(format!("\"{}\"", sym.bound_name));
        }
    }

    format!("load({})", parts.join(", "))
}

/// Check if a load statement spans multiple lines.
pub fn is_multiline_load<F>(load: &LoadStatement<'_>, line_end: F) -> bool
where
    F: Fn(ruff_text_size::TextSize) -> ruff_text_size::TextSize,
{
    if load.symbols.is_empty() {
        return false;
    }
    // Check if module path and first symbol are on different lines
    let module_line_end = line_end(load.module_path_range.start());
    let first_sym_line_end = line_end(load.symbols[0].range.start());
    module_line_end != first_sym_line_end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconstruct_load_positional_only() {
        let symbols = vec![
            LoadSymbol {
                bound_name: "a_func",
                original_name: None,
                range: TextRange::default(),
            },
            LoadSymbol {
                bound_name: "b_func",
                original_name: None,
                range: TextRange::default(),
            },
        ];
        assert_eq!(
            reconstruct_load(":defs.bzl", &symbols),
            "load(\":defs.bzl\", \"a_func\", \"b_func\")"
        );
    }

    #[test]
    fn test_reconstruct_load_with_keywords() {
        let symbols = vec![
            LoadSymbol {
                bound_name: "func",
                original_name: None,
                range: TextRange::default(),
            },
            LoadSymbol {
                bound_name: "alias",
                original_name: Some("original"),
                range: TextRange::default(),
            },
        ];
        assert_eq!(
            reconstruct_load(":defs.bzl", &symbols),
            "load(\":defs.bzl\", \"func\", alias=\"original\")"
        );
    }

    #[test]
    fn test_reconstruct_load_empty() {
        assert_eq!(reconstruct_load(":defs.bzl", &[]), "");
    }
}
