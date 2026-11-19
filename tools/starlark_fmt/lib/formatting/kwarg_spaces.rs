/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Adds spaces around `=` in kwargs and default params for buildifier compatibility.
//!
//! After ruff formatting, kwargs/defaults use PEP 8 style (`name=value`),
//! but buildifier uses spaced style (`name = value`).
//!
//! ```starlark
//! # Before (ruff style):
//! func(name="foo", deps=[])
//! def f(x=1): pass
//!
//! # After (buildifier style):
//! func(name = "foo", deps = [])
//! def f(x = 1): pass
//! ```

use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::visitor::walk_expr;
use ruff_python_ast::visitor::walk_stmt;
use ruff_text_size::Ranged;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;

use crate::autofixes::parsed_module::Edit;
use crate::autofixes::parsed_module::ParsedModule;

/// Collects positions of `=` in kwargs/defaults that need spacing.
struct KwargEqualsCollector<'a> {
    source: &'a str,
    positions: Vec<TextSize>,
}

impl<'a> KwargEqualsCollector<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            positions: Vec::new(),
        }
    }

    fn record_equals(&mut self, name_end: TextSize, value_start: TextSize) {
        let start = name_end.to_usize();
        let end = value_start.to_usize();
        if start >= end || end > self.source.len() {
            return;
        }
        if let Some(offset) = self.source[start..end].find('=') {
            let pos = TextSize::from((start + offset) as u32);
            let before = self.source.as_bytes().get(pos.to_usize().saturating_sub(1));
            let after = self.source.as_bytes().get(pos.to_usize() + 1);
            if before != Some(&b' ') || after != Some(&b' ') {
                self.positions.push(pos);
            }
        }
    }

    fn check_parameters(&mut self, params: &ruff_python_ast::Parameters) {
        for p in params
            .args
            .iter()
            .chain(&params.kwonlyargs)
            .chain(&params.posonlyargs)
        {
            if let Some(default) = &p.default {
                self.record_equals(p.parameter.name.range().end(), default.range().start());
            }
        }
    }
}

impl<'a> Visitor<'a> for KwargEqualsCollector<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let Stmt::FunctionDef(func) = stmt {
            self.check_parameters(&func.parameters);
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(call) => {
                for kw in &call.arguments.keywords {
                    if let Some(name) = &kw.arg {
                        let name_end =
                            TextSize::from((kw.range().start().to_usize() + name.len()) as u32);
                        self.record_equals(name_end, kw.value.range().start());
                    }
                }
            }
            Expr::Lambda(lambda) => {
                if let Some(params) = &lambda.parameters {
                    self.check_parameters(params);
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

/// Collect edits that add spaces around `=` in kwargs and default params.
pub fn collect_edits(module: &ParsedModule) -> Vec<Edit> {
    let source = module.source();
    if source.is_empty() {
        return Vec::new();
    }

    let mut collector = KwargEqualsCollector::new(source);
    for stmt in module.stmts() {
        collector.visit_stmt(stmt);
    }

    if collector.positions.is_empty() {
        return Vec::new();
    }

    collector.positions.sort();
    collector.positions.dedup();

    // Convert positions to edits: replace `=` with ` = `
    collector
        .positions
        .into_iter()
        .filter_map(|pos| {
            let i = pos.to_usize();
            let before = source.as_bytes().get(i.saturating_sub(1));
            let after = source.as_bytes().get(i + 1);

            let replacement = match (before != Some(&b' '), after != Some(&b' ')) {
                (true, true) => " = ",
                (true, false) => " =",
                (false, true) => "= ",
                (false, false) => return None,
            };

            Some(Edit::new(
                TextRange::new(pos, pos + TextSize::from(1)),
                replacement,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    fn run(source: &str) -> String {
        ParsedModule::parse(Cow::Borrowed(source))
            .and_then(|module| module.run_transform(collect_edits))
            .expect("failed")
            .unparse()
    }

    #[test]
    fn test_basic_kwarg() {
        assert_eq!(
            run("func(name=\"foo\", deps=[])\n"),
            "func(name = \"foo\", deps = [])\n"
        );
    }

    #[test]
    fn test_already_spaced() {
        let input = "func(name = \"foo\")\n";
        assert_eq!(run(input), input);
    }

    #[test]
    fn test_default_param() {
        assert_eq!(
            run("def f(x=1, y=\"foo\"):\n    pass\n"),
            "def f(x = 1, y = \"foo\"):\n    pass\n"
        );
    }

    #[test]
    fn test_lambda_default() {
        assert_eq!(run("f = lambda x=1: x\n"), "f = lambda x = 1: x\n");
    }

    #[test]
    fn test_nested_calls() {
        assert_eq!(
            run("outer(inner(a=1), b=2)\n"),
            "outer(inner(a = 1), b = 2)\n"
        );
    }

    #[test]
    fn test_preserves_assignment() {
        // Regular assignment should not be touched (already spaced by ruff)
        let input = "x = 1\n";
        assert_eq!(run(input), input);
    }

    #[test]
    fn test_preserves_comparisons() {
        // Comparison operators should not be touched
        let input = "x == y\n";
        assert_eq!(run(input), input);
    }

    #[test]
    fn test_string_contents_unchanged() {
        // = inside string should not be touched (AST doesn't see it as kwarg)
        let input = "x = \"foo=bar\"\n";
        assert_eq!(run(input), input);
    }

    #[test]
    fn test_multiline() {
        assert_eq!(
            run("func(\n    name=\"foo\",\n    deps=[],\n)\n"),
            "func(\n    name = \"foo\",\n    deps = [],\n)\n"
        );
    }
}
