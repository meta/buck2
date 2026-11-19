/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Collapse calls whose only argument is a collection expression
//! (dict, list, set, or comprehension), from:
//!
//! ```starlark
//! some_call(
//!     {
//!         "DEFAULT": [],
//!     }
//! )
//! ```
//!
//! to:
//!
//! ```starlark
//! some_call({
//!     "DEFAULT": [],
//! })
//! ```

use ruff_python_ast::Expr;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Stmt;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::visitor::walk_expr;
use ruff_python_ast::visitor::walk_stmt;
use ruff_text_size::Ranged;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;

use crate::autofixes::parsed_module::Edit;
use crate::autofixes::parsed_module::ParsedModule;

struct SingleLiteralCallCollector<'a> {
    source: &'a str,
    collapsed_call_ranges: Vec<TextRange>,
    edits: Vec<Edit>,
}

impl<'a> SingleLiteralCallCollector<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            collapsed_call_ranges: Vec::new(),
            edits: Vec::new(),
        }
    }

    fn process_call(&mut self, call: &ExprCall) {
        if !call.arguments.keywords.is_empty() || call.arguments.args.len() != 1 {
            return;
        }

        if self
            .collapsed_call_ranges
            .iter()
            .any(|range| range.contains_range(call.range()))
        {
            return;
        }

        let Some(literal) = call
            .arguments
            .args
            .first()
            .filter(|arg| is_collection_expr(arg))
        else {
            return;
        };

        // Dedenting explicit line continuations can change string literal contents:
        // indentation after `\` is part of the continued logical line.
        if contains_explicit_line_continuation(self.source, literal.range())
            || contains_triple_quoted_string(self.source, literal.range())
        {
            return;
        }

        let func_end = call.func.range().end();
        let literal_start = literal.range().start();
        let literal_end = literal.range().end();
        let call_end = call.range().end();

        let Some((opening_edit, indent_to_remove)) = collapse_opening(
            self.source,
            call.func.range().start(),
            func_end,
            literal_start,
        ) else {
            return;
        };
        let Some(closing_edit) = collapse_closing(self.source, literal_end, call_end) else {
            return;
        };
        let Some(mut dedent_edits) =
            collect_dedent_edits(self.source, literal_start, literal_end, indent_to_remove)
        else {
            return;
        };

        self.edits.push(opening_edit);
        self.edits.append(&mut dedent_edits);
        self.edits.push(closing_edit);
        self.collapsed_call_ranges.push(call.range());
    }
}

impl<'a> Visitor<'a> for SingleLiteralCallCollector<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            self.process_call(call);
        }
        walk_expr(self, expr);
    }
}

fn is_collection_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Dict(_)
            | Expr::DictComp(_)
            | Expr::List(_)
            | Expr::ListComp(_)
            | Expr::Set(_)
            | Expr::SetComp(_)
    )
}

fn contains_explicit_line_continuation(source: &str, range: TextRange) -> bool {
    source[range.start().to_usize()..range.end().to_usize()].contains("\\\n")
}

fn contains_triple_quoted_string(source: &str, range: TextRange) -> bool {
    let text = &source[range.start().to_usize()..range.end().to_usize()];
    text.contains("\"\"\"") || text.contains("'''")
}

fn collapse_opening<'a>(
    source: &'a str,
    func_start: TextSize,
    func_end: TextSize,
    literal_start: TextSize,
) -> Option<(Edit, &'a str)> {
    let between = &source[func_end.to_usize()..literal_start.to_usize()];
    let trimmed = between.trim();
    if trimmed != "(" || between == "(" {
        return None;
    }

    let indent_start = between.rfind('\n').map(|pos| pos + 1)?;
    let literal_line_indent = &between[indent_start..];
    let func_line_start = source[..func_start.to_usize()]
        .rfind('\n')
        .map(|pos| pos + 1)
        .unwrap_or(0);
    let func_line = &source[func_line_start..func_start.to_usize()];
    let func_line_indent_len = func_line
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == b' ' || **byte == b'\t')
        .count();

    if !literal_line_indent.starts_with(&func_line[..func_line_indent_len]) {
        return None;
    }

    let indent_to_remove = &literal_line_indent[func_line_indent_len..];
    if indent_to_remove.is_empty() {
        return None;
    }

    Some((
        Edit::new(TextRange::new(func_end, literal_start), "("),
        indent_to_remove,
    ))
}

fn collapse_closing(source: &str, literal_end: TextSize, call_end: TextSize) -> Option<Edit> {
    let between = &source[literal_end.to_usize()..call_end.to_usize()];
    let trimmed = between.trim();
    if trimmed != ")" || between == ")" {
        return None;
    }

    Some(Edit::new(TextRange::new(literal_end, call_end), ")"))
}

fn collect_dedent_edits(
    source: &str,
    literal_start: TextSize,
    literal_end: TextSize,
    indent_to_remove: &str,
) -> Option<Vec<Edit>> {
    let mut edits = Vec::new();
    let mut line_start = source[literal_start.to_usize()..literal_end.to_usize()]
        .find('\n')
        .map(|offset| literal_start.to_usize() + offset + 1)?;

    while line_start < literal_end.to_usize() {
        let line_end = source[line_start..]
            .find('\n')
            .map(|offset| line_start + offset)
            .unwrap_or(source.len());
        let line = &source[line_start..line_end];

        if !line.trim().is_empty() {
            if !line.starts_with(indent_to_remove) {
                return None;
            }

            let start = TextSize::from(line_start as u32);
            edits.push(Edit::delete(TextRange::new(
                start,
                start + TextSize::from(indent_to_remove.len() as u32),
            )));
        }

        line_start = line_end + 1;
    }

    Some(edits)
}

/// Collect edits that collapse calls whose only argument is a collection expression.
pub fn collect_edits(module: &ParsedModule) -> Vec<Edit> {
    if module.source().is_empty() {
        return Vec::new();
    }

    let mut collector = SingleLiteralCallCollector::new(module.source());
    for stmt in module.stmts() {
        collector.visit_stmt(stmt);
    }
    collector.edits
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use indoc::indoc;

    use super::*;
    use crate::autofixes::parsed_module::ParsedModule;

    fn apply(input: &str) -> String {
        ParsedModule::parse(Cow::Owned(input.to_owned()))
            .and_then(|module| module.run_transform(collect_edits))
            .expect("transform failed")
            .unparse()
    }

    #[test]
    fn test_select_dict_call_collapses() {
        let input = indoc! {r#"
            x = select(
                {
                    "DEFAULT": [],
                }
            )
        "#};
        let expected = indoc! {r#"
            x = select({
                "DEFAULT": [],
            })
        "#};
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_list_call_collapses() {
        let input = indoc! {r#"
            x = deps(
                [
                    "//foo:bar",
                ]
            )
        "#};
        let expected = indoc! {r#"
            x = deps([
                "//foo:bar",
            ])
        "#};
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_list_comprehension_call_collapses() {
        let input = indoc! {r#"
            x = " ".join(
                [
                    item
                    for item in items
                ]
            )
        "#};
        let expected = indoc! {r#"
            x = " ".join([
                item
                for item in items
            ])
        "#};
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_explicit_line_continuation_unchanged() {
        let input = indoc! {r#"
            x = " ".join(
                [
                    "mkdir -p ${OUT}/; \
                    cp -R $(location {}) ${OUT}/".format(target)
                    for target in targets
                ]
            )
        "#};
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_triple_quoted_string_unchanged() {
        let input = indoc! {r###"
            x = "\n".join(
                [
                    """TEST(Module, ModuleName) {
                        EXPECT_EQ(
                            actual,
                            expected);
                    }""".format(name),
                ]
            )
        "###};
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_collapses_select_single_list_call() {
        let input = indoc! {r#"
            x = select(
                [
                    "//foo:bar",
                ]
            )
        "#};
        let expected = indoc! {r#"
            x = select([
                "//foo:bar",
            ])
        "#};
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_set_call_collapses() {
        let input = indoc! {r#"
            x = tags(
                {
                    "manual",
                }
            )
        "#};
        let expected = indoc! {r#"
            x = tags({
                "manual",
            })
        "#};
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_collapses_select_single_set_call() {
        let input = indoc! {r#"
            x = select(
                {
                    "manual",
                }
            )
        "#};
        let expected = indoc! {r#"
            x = select({
                "manual",
            })
        "#};
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_any_function_collapses() {
        let input = indoc! {r#"
            x = choose(
                {
                    "DEFAULT": [],
                }
            )
        "#};
        let expected = indoc! {r#"
            x = choose({
                "DEFAULT": [],
            })
        "#};
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_already_collapsed_unchanged() {
        let input = indoc! {r#"
            x = select({
                "DEFAULT": [],
            })
        "#};
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_trailing_comma_unchanged() {
        let input = indoc! {r#"
            x = select(
                {
                    "DEFAULT": [],
                },
            )
        "#};
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_comments_between_parens_unchanged() {
        let input = indoc! {r#"
            x = select(
                # keep comment
                {
                    "DEFAULT": [],
                }
            )
        "#};
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_multiple_args_unchanged() {
        let input = indoc! {r#"
            x = select(
                {
                    "DEFAULT": [],
                },
                y,
            )
        "#};
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_kwargs_unchanged() {
        let input = indoc! {r#"
            x = select(
                {
                    "DEFAULT": [],
                },
                mode = "x",
            )
        "#};
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_non_collection_literal_unchanged() {
        let input = indoc! {r#"
            x = maybe(
                "value"
            )
        "#};
        assert_eq!(apply(input), input);
    }
}
