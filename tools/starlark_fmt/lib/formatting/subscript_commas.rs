/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Remove trailing commas from subscript expressions like `dict[str, int,]`.
//!
//! Go-based Starlark interpreters don't accept trailing commas inside subscript
//! expressions (e.g., `dict[str, typing.Any,]`). This pass walks the AST, finds
//! every subscript, and emits edits to strip trailing commas.

use ruff_python_ast::Expr;
use ruff_python_ast::ExprSubscript;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::visitor::walk_expr;
use ruff_text_size::Ranged;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;

use crate::autofixes::parsed_module::Edit;
use crate::autofixes::parsed_module::ParsedModule;

struct SubscriptCommaCollector<'a> {
    source: &'a str,
    edits: Vec<Edit>,
}

impl<'a> SubscriptCommaCollector<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            edits: Vec::new(),
        }
    }

    fn process_subscript(&mut self, sub: &ExprSubscript) {
        // For a subscript like `dict[str, int,]`, the slice is the expression inside
        // the brackets. For Tuple slices, ruff's range includes the trailing comma,
        // so we use the last element's end to find the comma between it and `]`.
        if let Expr::Tuple(tuple) = &*sub.slice {
            if let Some(last) = tuple.elts.last() {
                self.maybe_strip_trailing_comma(last.range().end(), sub.range().end());
            }
        } else {
            self.maybe_strip_trailing_comma(sub.slice.range().end(), sub.range().end());
        }

        // Special handling for typing.Callable[[args...], ret] which has the form:
        // Subscript(Callable, Tuple([List([args...]), ret]))
        // We need to also strip trailing commas from the inner List of args.
        if is_typing_callable(&sub.value) {
            self.process_callable(sub);
        }
    }

    fn process_callable(&mut self, sub: &ExprSubscript) {
        // Slice should be `Tuple([List([args...]), return_type])`.
        let Expr::Tuple(outer) = &*sub.slice else {
            return;
        };
        let [Expr::List(inner), _ret] = outer.elts.as_slice() else {
            return;
        };

        if let Some(last) = inner.elts.last() {
            self.maybe_strip_trailing_comma(last.range().end(), inner.range().end());
        }
    }

    /// Emit a delete-edit for a `,` that lies between `after` and the closing
    /// bracket at position `bracket_end - 1`, separated only by whitespace.
    fn maybe_strip_trailing_comma(&mut self, after: TextSize, bracket_end: TextSize) {
        let bracket_pos = bracket_end.to_usize().saturating_sub(1);
        let after_pos = after.to_usize();
        if after_pos >= bracket_pos {
            return;
        }
        let span = &self.source[after_pos..bracket_pos];
        let Some(comma_offset) = span.find(',') else {
            return;
        };
        if !span[..comma_offset].chars().all(char::is_whitespace) {
            return;
        }
        let pos = TextSize::from((after_pos + comma_offset) as u32);
        self.edits
            .push(Edit::delete(TextRange::new(pos, pos + TextSize::from(1))));
    }
}

impl<'a> Visitor<'a> for SubscriptCommaCollector<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Subscript(sub) = expr {
            self.process_subscript(sub);
        }
        walk_expr(self, expr);
    }
}

/// True if `expr` is the attribute access `typing.Callable`.
fn is_typing_callable(expr: &Expr) -> bool {
    let Expr::Attribute(attr) = expr else {
        return false;
    };
    if attr.attr.as_str() != "Callable" {
        return false;
    }
    let Expr::Name(name) = &*attr.value else {
        return false;
    };
    name.id.as_str() == "typing"
}

pub fn collect_edits(module: &ParsedModule) -> Vec<Edit> {
    let source = module.source();
    if source.is_empty() {
        return Vec::new();
    }

    let mut collector = SubscriptCommaCollector::new(source);
    for stmt in module.stmts() {
        collector.visit_stmt(stmt);
    }
    collector.edits
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::autofixes::parsed_module::ParsedModule;

    fn apply(input: &str) -> String {
        let module = ParsedModule::parse(Cow::Owned(input.to_owned())).expect("parse failed");
        let edits = collect_edits(&module);
        module
            .run_transform(|_| edits)
            .expect("transform failed")
            .unparse()
    }

    #[test]
    fn test_basic_trailing_comma() {
        let input = "x = typing.Callable[[A, B,], C]\n";
        let expected = "x = typing.Callable[[A, B], C]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_multiline_trailing_comma() {
        let input = "x = typing.Callable[[\n    A,\n    B,\n], C]\n";
        let expected = "x = typing.Callable[[\n    A,\n    B\n], C]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_no_trailing_comma_unchanged() {
        let input = "x = typing.Callable[[A, B], C]\n";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_multiline_no_trailing_comma_unchanged() {
        let input = "x = typing.Callable[[\n    A,\n    B\n], C]\n";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_nested_dict_with_any() {
        // The PikaSelectSpecType case
        let input = "typing.Callable[[\n    AppleToolchainRuleType,\n    dict[str, typing.Any],\n], typing.Any]\n";
        let expected = "typing.Callable[[\n    AppleToolchainRuleType,\n    dict[str, typing.Any]\n], typing.Any]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_multiple_callables() {
        let input = "x = typing.Callable[[A,], B]\ny = typing.Callable[[C,], D]\n";
        let expected = "x = typing.Callable[[A], B]\ny = typing.Callable[[C], D]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_regular_list_unchanged() {
        // Regular lists should not be affected
        let input = "x = [A, B,]\n";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_nested_callable() {
        // Nested structures within Callable
        let input = "x = typing.Callable[[dict[str, int], list[str],], bool]\n";
        let expected = "x = typing.Callable[[dict[str, int], list[str]], bool]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_callable_with_complex_types() {
        let input = "x = typing.Callable[[\n    typing.Callable[[int], str],\n    dict[str, typing.Any],\n], None]\n";
        let expected = "x = typing.Callable[[\n    typing.Callable[[int], str],\n    dict[str, typing.Any]\n], None]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_empty_callable() {
        let input = "x = typing.Callable[[], C]\n";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_single_element_with_trailing_comma() {
        let input = "x = typing.Callable[[A,], C]\n";
        let expected = "x = typing.Callable[[A], C]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_idempotent() {
        let input = "x = typing.Callable[[\n    A,\n    B,\n], C]\n";
        let first = apply(input);
        let second = apply(&first);
        assert_eq!(first, second, "should be idempotent");
    }

    #[test]
    fn test_pika_select_spec_type() {
        // Real-world example from the issue
        let input = r#"PikaSelectSpecType = record(
    select_value_fn = typing.Callable[[
        AppleToolchainRuleType,
        ExecutionHostOSType | None,
        str,
        PikaAssertsModeType | None,
        AppleSdk | None,
        AppleArch | None,
        dict[str, typing.Any],
    ], typing.Any],
    select_value_fn_kwargs = dict[str, typing.Any],
    default_asserts_mode = PikaAssertsModeType,
    default_pika_version = str,
)
"#;
        let expected = r#"PikaSelectSpecType = record(
    select_value_fn = typing.Callable[[
        AppleToolchainRuleType,
        ExecutionHostOSType | None,
        str,
        PikaAssertsModeType | None,
        AppleSdk | None,
        AppleArch | None,
        dict[str, typing.Any]
    ], typing.Any],
    select_value_fn_kwargs = dict[str, typing.Any],
    default_asserts_mode = PikaAssertsModeType,
    default_pika_version = str,
)
"#;
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_outer_list_trailing_comma() {
        // Trailing comma after return type in outer list
        let input = "x = typing.Callable[[A, B], C,]\n";
        let expected = "x = typing.Callable[[A, B], C]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_both_inner_and_outer_trailing_commas() {
        // Both inner list and outer list have trailing commas
        let input = "x = typing.Callable[[\n    A,\n    B,\n], C,\n]\n";
        let expected = "x = typing.Callable[[\n    A,\n    B\n], C\n]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_dict_with_trailing_comma() {
        // Test case from the issue: dict[str, [foo, bar],] should work
        let input = "x = dict[str, [foo, bar],]\n";
        let expected = "x = dict[str, [foo, bar]]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_nested_dict_trailing_comma() {
        // Nested dict with trailing comma
        let input = "x = dict[str, dict[str, int],]\n";
        let expected = "x = dict[str, dict[str, int]]\n";
        assert_eq!(apply(input), expected);
    }

    #[test]
    fn test_simple_dict_trailing_comma() {
        // Simple dict subscript with trailing comma
        // Note: dict[str, int] as a value might be parsed differently
        // This test uses a subscript that is definitely parsed as Expr::Subscript
        let input = "x = foo[bar, baz,]\n";
        let expected = "x = foo[bar, baz]\n";
        assert_eq!(apply(input), expected);
    }
}
