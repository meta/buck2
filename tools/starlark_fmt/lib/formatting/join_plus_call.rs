/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Keep configured calls joined to a preceding `+`.
//!
//! ```starlark
//! ["A"]
//! + select({...})
//! ```
//!
//! becomes:
//!
//! ```starlark
//! ["A"] + select({...})
//! ```

use ruff_python_ast::Expr;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::visitor::walk_expr;
use ruff_python_ast::visitor::walk_stmt;
use ruff_text_size::Ranged;
use ruff_text_size::TextRange;

use crate::autofixes::parsed_module::Edit;
use crate::autofixes::parsed_module::ParsedModule;

struct PlusCallJoiner<'a, 'b> {
    source: &'a str,
    joinable_functions: &'b [&'b str],
    edits: Vec<Edit>,
    /// How many `+` BinOps deep we are. Only `+` BinOps increment this
    /// counter, so a `+` nested inside a non-`+` operator (e.g. `x * (a + b)`)
    /// remains at depth 0 and is still eligible for joining — correctly, since
    /// it is a standalone two-term `+`, not part of a longer `+` chain.
    ///
    /// Inner `+`s in a chain (e.g. `a + select(...) + b` parses as
    /// `(a + select(...)) + b`) must NOT be joined — keeping the chain laid
    /// out on separate lines is more readable than collapsing the middle
    /// `+ select(...)` and leaving the rest dangling. The visitor uses this to
    /// skip `process_expr` on any inner `+` while still walking through
    /// children.
    add_binop_depth: u32,
}

impl<'a, 'b> PlusCallJoiner<'a, 'b> {
    fn new(source: &'a str, joinable_functions: &'b [&'b str]) -> Self {
        Self {
            source,
            joinable_functions,
            edits: Vec::new(),
            add_binop_depth: 0,
        }
    }

    fn process_expr(&mut self, expr: &Expr) {
        let Expr::BinOp(binop) = expr else {
            return;
        };
        if !matches!(binop.op, Operator::Add) {
            return;
        }

        // Bail if this `+` is part of a multi-term chain. The depth guard in
        // `visit_expr` handles inner nodes (depth > 0), but this check catches
        // the outermost `+` (depth == 0) whose operand is itself a `+` BinOp.
        // Starlark parses `+` left-to-right, so `a + b + c` is `(a + b) + c`:
        // the outer node has a left operand that is a `+` BinOp.
        if is_add_binop(binop.left.as_ref()) || is_add_binop(binop.right.as_ref()) {
            return;
        }

        let Expr::Call(call) = binop.right.as_ref() else {
            return;
        };
        if !self.is_joinable_call(call) {
            return;
        }

        let between_range = TextRange::new(binop.left.range().end(), call.range().start());
        let between = &self.source[between_range];
        if between == " + " || between.trim() != "+" {
            return;
        }

        self.edits.push(Edit::new(between_range, " + "));
    }

    fn is_joinable_call(&self, call: &ExprCall) -> bool {
        let Expr::Name(name) = call.func.as_ref() else {
            return false;
        };

        self.joinable_functions
            .iter()
            .any(|function| *function == name.id.as_str())
    }
}

fn is_add_binop(expr: &Expr) -> bool {
    matches!(expr, Expr::BinOp(b) if matches!(b.op, Operator::Add))
}

impl<'a> Visitor<'a> for PlusCallJoiner<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        let is_add = is_add_binop(expr);

        if self.add_binop_depth == 0 {
            self.process_expr(expr);
        }

        if is_add {
            self.add_binop_depth += 1;
        }
        walk_expr(self, expr);
        if is_add {
            self.add_binop_depth -= 1;
        }
    }
}

/// Collect edits that keep `+ configured_call(...)` on the preceding line.
pub fn collect_edits(module: &ParsedModule, joinable_functions: &[&str]) -> Vec<Edit> {
    if module.source().is_empty() || joinable_functions.is_empty() {
        return Vec::new();
    }

    let mut joiner = PlusCallJoiner::new(module.source(), joinable_functions);
    for stmt in module.stmts() {
        joiner.visit_stmt(stmt);
    }
    joiner.edits
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use indoc::indoc;

    use super::*;
    use crate::autofixes::parsed_module::ParsedModule;

    fn apply(input: &str, joinable_functions: &[&str]) -> String {
        ParsedModule::parse(Cow::Owned(input.to_owned()))
            .and_then(|module| {
                module.run_transform(|module| collect_edits(module, joinable_functions))
            })
            .expect("transform failed")
            .unparse()
    }

    #[test]
    fn test_joins_configured_call_after_plus() {
        let input = indoc! {r#"
            x = (
                ["A"]
                + select({"DEFAULT": []})
            )
        "#};
        let expected = indoc! {r#"
            x = (
                ["A"] + select({"DEFAULT": []})
            )
        "#};
        assert_eq!(apply(input, &["select"]), expected);
    }

    #[test]
    fn test_joins_custom_configured_call_after_plus() {
        let input = indoc! {r#"
            x = (
                ["A"]
                + choose({"DEFAULT": []})
            )
        "#};
        let expected = indoc! {r#"
            x = (
                ["A"] + choose({"DEFAULT": []})
            )
        "#};
        assert_eq!(apply(input, &["choose"]), expected);
    }

    #[test]
    fn test_unconfigured_call_unchanged() {
        let input = indoc! {r#"
            x = (
                ["A"]
                + choose({"DEFAULT": []})
            )
        "#};
        assert_eq!(apply(input, &["select"]), input);
    }

    #[test]
    fn test_comment_between_plus_and_call_unchanged() {
        let input = indoc! {r#"
            x = (
                ["A"]
                +  # keep
                select({"DEFAULT": []})
            )
        "#};
        assert_eq!(apply(input, &["select"]), input);
    }

    #[test]
    fn test_already_joined_unchanged() {
        let input = "x = [\"A\"] + select({\"DEFAULT\": []})\n";
        assert_eq!(apply(input, &["select"]), input);
    }

    #[test]
    fn test_plus_chain_with_trailing_term_unchanged() {
        // Starlark `+` is left-associative, so `["A"] + select({...}) + ANOTHER`
        // parses as `(["A"] + select({...})) + ANOTHER`. When we visit the
        // outer `+`, its left operand is a `+` BinOp, so the operand check in
        // `process_expr` bails. The inner `["A"] + select({...})` is skipped
        // by the depth guard (depth > 0). Net effect: no joining.
        let input = indoc! {r#"
            x = (
                ["A"]
                + select({"DEFAULT": []})
                + ANOTHER
            )
        "#};
        assert_eq!(apply(input, &["select"]), input);
    }

    #[test]
    fn test_plus_chain_with_leading_term_unchanged() {
        // Same reasoning when the configured call is in the leading position
        // of a `+` chain.
        let input = indoc! {r#"
            x = (
                LEADING
                + select({"DEFAULT": []})
                + ["B"]
            )
        "#};
        assert_eq!(apply(input, &["select"]), input);
    }

    #[test]
    fn test_plus_inside_non_plus_binop_still_joined() {
        // A `+` nested inside a non-`+` operator (e.g. inside a tuple or
        // separate expression) is NOT part of a `+` chain — only `+` BinOps
        // increment add_binop_depth. This `+` remains at depth 0 and is joined.
        let input = indoc! {r#"
            x = (
                ["A"]
                + select({"DEFAULT": []})
            )
        "#};
        let expected = indoc! {r#"
            x = (
                ["A"] + select({"DEFAULT": []})
            )
        "#};
        assert_eq!(apply(input, &["select"]), expected);
    }

    #[test]
    fn test_plus_chain_of_only_configured_calls_unchanged() {
        let input = indoc! {r#"
            x = (
                select({"a": []})
                + select({"b": []})
                + select({"c": []})
            )
        "#};
        assert_eq!(apply(input, &["select"]), input);
    }
}
