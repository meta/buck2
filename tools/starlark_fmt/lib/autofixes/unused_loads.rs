/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Removes unused symbols from load statements.
//!
//! For each load statement, checks whether the bound names are referenced
//! elsewhere in the module. Unused symbols are removed from the load.
//! If all symbols are unused, the entire load statement is removed.
//!
//! Symbols annotated with `@unused` in their trailing comment are preserved:
//! ```starlark
//! load(
//!     ":defs.bzl",
//!     "Type",  # @unused
//! )
//! ```
//!
//! ```starlark
//! # Before:
//! load(":defs.bzl", "foo", "used", bar="baz")
//! x = used()
//!
//! # After:
//! load(":defs.bzl", "used")
//! x = used()
//! ```

use std::collections::HashSet;

use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::visitor::walk_expr;
use ruff_python_ast::visitor::walk_stmt;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;

use super::load_utils::LoadStatement;
use super::load_utils::extract_load_statements;
use super::load_utils::is_load_call;
use super::load_utils::is_multiline_load;
use super::load_utils::reconstruct_load;
use super::parsed_module::Edit;
use super::parsed_module::ParsedModule;

/// Visitor that collects all name usages in the module.
struct NameCollector<'a> {
    used_names: HashSet<&'a str>,
}

impl<'a> Visitor<'a> for NameCollector<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Name(name) = expr {
            self.used_names.insert(name.id.as_str());
        }
        walk_expr(self, expr);
    }
}

/// Check if a symbol has an @unused annotation in its trailing comment.
/// The comment must be on the same line as the symbol, INSIDE the load() call.
/// We detect statement-level comments by checking if the symbol ends before the
/// comment would be inside the statement (i.e., the comment range starts before stmt ends).
fn has_unused_annotation(
    symbol_range: TextRange,
    stmt_range: TextRange,
    module: &ParsedModule,
) -> bool {
    // Get the line end of this symbol
    let line_end = module.line_end(symbol_range.end());
    let search_range = TextRange::new(symbol_range.end(), line_end);

    // Check for comments on the same line after the symbol
    for comment_text in module.comments_in_range(search_range) {
        if comment_text.contains("@unused") {
            // The key insight: if symbol_range.end() is close to stmt_range.end(),
            // this is likely the last symbol and the comment is statement-level.
            // For multiline loads, symbols are NOT on the same line as the closing paren,
            // so their line_end will be different from where the stmt ends.
            // For single-line loads like `load(":a.bzl", "x")  # @unused`,
            // the symbol ends at the `"` and stmt ends after `)  # @unused\n`,
            // but the comment is AFTER the `)`.
            //
            // We can approximate: if the statement ends on the same line as the symbol,
            // then this is a single-line load and the comment is statement-level.
            // Otherwise (multiline), the comment is symbol-level.
            let stmt_line_end = module.line_end(stmt_range.start());
            let symbol_line_end = module.line_end(symbol_range.start());

            if stmt_line_end == symbol_line_end {
                // Single-line load - comment is statement-level, not symbol-level
                return false;
            }

            return true;
        }
    }

    false
}

/// Collect all names that are actually used in the module (excluding load statements).
fn collect_used_names<'a>(stmts: &'a [Stmt]) -> HashSet<&'a str> {
    let mut collector = NameCollector {
        used_names: HashSet::new(),
    };

    for stmt in stmts {
        // Skip load statements when collecting usages
        if let Stmt::Expr(expr_stmt) = stmt {
            if is_load_call(&expr_stmt.value) {
                continue;
            }
        }
        walk_stmt(&mut collector, stmt);
    }

    collector.used_names
}

/// Compute the range to delete for an unused symbol in a multiline load.
/// This includes any leading comments/whitespace between the previous element and this symbol.
fn compute_deletion_range(
    sym_index: usize,
    load: &LoadStatement<'_>,
    module: &ParsedModule,
) -> TextRange {
    let sym = &load.symbols[sym_index];

    // Find where the previous element ends (including its line's newline)
    let prev_end = if sym_index == 0 {
        // First symbol - previous element is the module path
        // Delete from the line after the module path line
        let module_line = module.full_line_range(load.module_path_range.start());
        module_line.end()
    } else {
        // Previous symbol exists - delete from the line after it
        let prev_sym = &load.symbols[sym_index - 1];
        let prev_line = module.full_line_range(prev_sym.range.start());
        prev_line.end()
    };

    // Delete up to and including this symbol's line
    let sym_line = module.full_line_range(sym.range.start());

    TextRange::new(prev_end, sym_line.end())
}

/// Check if the entire load statement has a load-level @unused annotation.
/// This covers:
/// 1. Trailing comment on the statement line: `load(":a.bzl", "sym")  # @unused`
/// 2. Leading comment above the statement: `# @unused\nload(":a.bzl", "sym")`
///
/// When a load has a load-level @unused, ALL symbols are preserved regardless
/// of whether they are referenced elsewhere.
fn has_load_level_unused(load: &LoadStatement<'_>, module: &ParsedModule) -> bool {
    // Check trailing comment on the same line after the statement
    let stmt_line_end = module.line_end(load.stmt_range.end());
    let trailing_range = TextRange::new(load.stmt_range.end(), stmt_line_end);
    for comment_text in module.comments_in_range(trailing_range) {
        if comment_text.contains("@unused") {
            return true;
        }
    }

    // Check leading comment on the line immediately before the statement
    let stmt_line = module.full_line_range(load.stmt_range.start());
    if stmt_line.start() > TextSize::from(0) {
        // Go back one byte to get into the previous line
        let prev_line_offset = stmt_line.start() - TextSize::from(1);
        let prev_line = module.full_line_range(prev_line_offset);
        for comment_text in module.comments_in_range(prev_line) {
            if comment_text.contains("@unused") {
                return true;
            }
        }
    }

    false
}

/// Collect edits that remove unused load symbols.
pub fn collect_edits(module: &ParsedModule) -> Vec<Edit> {
    let stmts = module.stmts();
    if stmts.is_empty() {
        return Vec::new();
    }

    let load_stmts = extract_load_statements(stmts);
    if load_stmts.is_empty() {
        return Vec::new();
    }

    let used_names = collect_used_names(stmts);
    let mut edits: Vec<Edit> = Vec::new();

    for mut load in load_stmts {
        // Load-level @unused preserves the entire load statement
        if has_load_level_unused(&load, module) {
            continue;
        }
        let original_len = load.symbols.len();

        // For multiline loads, we need the original indices to compute deletion ranges
        if is_multiline_load(&load, |offset| module.line_end(offset)) {
            // Collect indices of unused symbols (in reverse order for stable deletion)
            let unused_indices: Vec<usize> = load
                .symbols
                .iter()
                .enumerate()
                .filter(|(_, sym)| {
                    !used_names.contains(sym.bound_name)
                        && !has_unused_annotation(sym.range, load.stmt_range, module)
                })
                .map(|(idx, _)| idx)
                .collect();

            if unused_indices.is_empty() {
                continue;
            }

            if unused_indices.len() == original_len {
                // All symbols unused - remove entire statement
                edits.push(Edit::delete(load.stmt_range));
            } else {
                // Delete individual ranges for unused args
                for idx in unused_indices {
                    let deletion_range = compute_deletion_range(idx, &load, module);
                    edits.push(Edit::delete(deletion_range));
                }
            }
        } else {
            // Single-line load: use retain and reconstruct
            load.symbols.retain(|sym| {
                used_names.contains(sym.bound_name)
                    || has_unused_annotation(sym.range, load.stmt_range, module)
            });

            if load.symbols.len() == original_len {
                // Nothing removed
                continue;
            }

            if load.symbols.is_empty() {
                // Remove entire statement
                edits.push(Edit::delete(load.stmt_range));
            } else {
                // Reconstruct with remaining symbols
                let new_load = reconstruct_load(load.module_path, &load.symbols);
                edits.push(Edit::new(load.stmt_range, new_load));
            }
        }
    }

    edits
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use indoc::indoc;

    use super::*;

    fn run(source: &str) -> String {
        ParsedModule::parse(Cow::Borrowed(source))
            .and_then(|module| module.run_transform(collect_edits))
            .expect("failed to collect edits")
            .unparse()
    }

    #[test]
    fn test_remove_unused_symbol() {
        let source = indoc! {r#"
            load(":a.bzl", "used", "unused")
            x = used()
        "#};
        let expected = indoc! {r#"
            load(":a.bzl", "used")
            x = used()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_remove_entire_load_when_all_unused() {
        let source = indoc! {r#"
            load(":a.bzl", "unused1", "unused2")
            x = 42
        "#};
        // Leaves newline behind - ruff formatter cleans this up
        assert_eq!(run(source), "\nx = 42\n");
    }

    #[test]
    fn test_keep_all_when_all_used() {
        let source = indoc! {r#"
            load(":a.bzl", "foo", "bar")
            x = foo(bar)
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_keyword_arg_unused() {
        let source = indoc! {r#"
            load(":a.bzl", renamed="original")
            x = 42
        "#};
        // Leaves newline behind - ruff formatter cleans this up
        assert_eq!(run(source), "\nx = 42\n");
    }

    #[test]
    fn test_keyword_arg_used() {
        let source = indoc! {r#"
            load(":a.bzl", renamed="original")
            x = renamed()
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_mixed_used_and_unused() {
        let source = indoc! {r#"
            load(":a.bzl", "foo", "used", bar="baz")
            x = used()
        "#};
        let expected = indoc! {r#"
            load(":a.bzl", "used")
            x = used()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_no_loads() {
        let source = "x = 42\n";
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_multiple_loads_partial_removal() {
        let source = indoc! {r#"
            load(":a.bzl", "a_used", "a_unused")
            load(":b.bzl", "b_unused")
            x = a_used()
        "#};
        let expected = indoc! {r#"
            load(":a.bzl", "a_used")

            x = a_used()
        "#};
        // Leaves newline behind - ruff formatter cleans this up
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_name_used_in_rule_call() {
        let source = indoc! {r#"
            load(":a.bzl", "my_rule")
            my_rule(
                name = "test",
            )
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_substring_not_matched() {
        let source = indoc! {r#"
            load(":a.bzl", "foo")
            x = foobar()
        "#};
        // Leaves newline behind - ruff formatter cleans this up
        assert_eq!(run(source), "\nx = foobar()\n");
    }

    #[test]
    fn test_preserves_non_load_statements() {
        let source = indoc! {r#"
            load(":a.bzl", "unused")
            foo(name = "bar")
            baz()
        "#};
        let expected = indoc! {r#"

            foo(name = "bar")
            baz()
        "#};
        // Leaves newline behind - ruff formatter cleans this up
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_all_loads_used() {
        let source = indoc! {r#"
            load(":a.bzl", "a")
            load(":b.bzl", "b")
            x = a(b)
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_constant_usage_preserved() {
        // Constants referenced by name should not be stripped
        let source = indoc! {r#"
            load(":constants.bzl", "MY_CONSTANT", "UNUSED_CONST")
            x = MY_CONSTANT
        "#};
        let expected = indoc! {r#"
            load(":constants.bzl", "MY_CONSTANT")
            x = MY_CONSTANT
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_constant_in_list() {
        // Constants used in a list should be preserved
        let source = indoc! {r#"
            load(":constants.bzl", "FOO", "BAR", "UNUSED")
            my_list = [FOO, BAR]
        "#};
        let expected = indoc! {r#"
            load(":constants.bzl", "FOO", "BAR")
            my_list = [FOO, BAR]
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_constant_in_dict() {
        // Constants used as dict values should be preserved
        let source = indoc! {r#"
            load(":constants.bzl", "VALUE", "UNUSED")
            my_dict = {"key": VALUE}
        "#};
        let expected = indoc! {r#"
            load(":constants.bzl", "VALUE")
            my_dict = {"key": VALUE}
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_struct_constructor_preserved() {
        // Struct constructors (called like functions) should be preserved
        let source = indoc! {r#"
            load(":structs.bzl", "MyStruct", "UnusedStruct")
            x = MyStruct(field = 42)
        "#};
        let expected = indoc! {r#"
            load(":structs.bzl", "MyStruct")
            x = MyStruct(field = 42)
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_struct_as_argument() {
        // Struct type passed as argument should be preserved
        let source = indoc! {r#"
            load(":types.bzl", "MyType", "UnusedType")
            register_type(MyType)
        "#};
        let expected = indoc! {r#"
            load(":types.bzl", "MyType")
            register_type(MyType)
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_enum_value_preserved() {
        // Enum-like values (attribute access on loaded name) should preserve the base
        let source = indoc! {r#"
            load(":enums.bzl", "Color", "UnusedEnum")
            x = Color.RED
        "#};
        let expected = indoc! {r#"
            load(":enums.bzl", "Color")
            x = Color.RED
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_module_attribute_access_preserved() {
        // Accessing attributes/methods on loaded module should preserve it
        let source = indoc! {r#"
            load(":defs.bzl", "foo", "unused")
            A = foo.bar()
        "#};
        let expected = indoc! {r#"
            load(":defs.bzl", "foo")
            A = foo.bar()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_type_annotation_preserves_load() {
        // Type used in function annotation should be preserved
        let source = indoc! {r#"
            load(":defs.bzl", "MyType", "UnusedType")
            def foo(a: MyType) -> str:
                return "HI"
        "#};
        let expected = indoc! {r#"
            load(":defs.bzl", "MyType")
            def foo(a: MyType) -> str:
                return "HI"
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_return_type_annotation_preserves_load() {
        // Type used in return annotation should be preserved
        let source = indoc! {r#"
            load(":defs.bzl", "ReturnType", "Unused")
            def foo() -> ReturnType:
                return None
        "#};
        let expected = indoc! {r#"
            load(":defs.bzl", "ReturnType")
            def foo() -> ReturnType:
                return None
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_unused_annotation_on_arg_preserves_symbol() {
        // Per-arg @unused in multiline load preserves that symbol
        let source = indoc! {r#"
            load(
                ":a.bzl",
                "unused",
                "Type",  # @unused
            )
            x = 42
        "#};
        let expected = indoc! {r#"
            load(
                ":a.bzl",
                "Type",  # @unused
            )
            x = 42
        "#};
        // "unused" removed, "Type" preserved due to @unused
        // Multiline format is preserved with comments intact
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_statement_level_unused_honored() {
        // Statement-level @unused preserves the entire load
        let source = indoc! {r#"
            load(":a.bzl", "unused")  # @unused
            x = 42
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_statement_level_unused_no_space() {
        // #@unused (no space) also preserves the load
        let source = indoc! {r#"
            load(":a.bzl", "unused")  #@unused
            x = 42
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_leading_comment_unused_honored() {
        // # @unused on line before load preserves the entire load
        let source = indoc! {r#"
            # @unused
            load(":a.bzl", "unused")
            x = 42
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_leading_comment_unused_multiline_load() {
        // # @unused on line before a multiline load preserves all symbols
        let source = indoc! {r#"
            # @unused
            load(
                ":a.bzl",
                "unused1",
                "unused2",
            )
            x = 42
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_inline_comment_on_used_arg_preserved() {
        // For multiline loads, we delete individual lines so comments on used args are preserved
        let source = indoc! {r#"
            load(
                ":a.bzl",
                "used",  # important comment
                "unused",
            )
            x = used()
        "#};
        let expected = indoc! {r#"
            load(
                ":a.bzl",
                "used",  # important comment
            )
            x = used()
        "#};
        // Comment is preserved since we only delete the "unused" line
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_leading_comment_on_used_arg_preserved() {
        // Leading comments on args are also preserved for multiline loads
        let source = indoc! {r#"
            load(
                ":a.bzl",
                # This is an important symbol
                "used",
                "unused",
            )
            x = used()
        "#};
        let expected = indoc! {r#"
            load(
                ":a.bzl",
                # This is an important symbol
                "used",
            )
            x = used()
        "#};
        // Leading comment is preserved
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_inline_comment_on_unused_arg_removed() {
        // Comments on unused args are correctly removed (since the line is removed)
        let source = indoc! {r#"
            load(
                ":a.bzl",
                "used",
                "unused",  # this will be removed
            )
            x = used()
        "#};
        let expected = indoc! {r#"
            load(
                ":a.bzl",
                "used",
            )
            x = used()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_leading_comment_on_unused_arg_removed() {
        // Leading comments on unused args are also removed
        // We delete from the end of the previous symbol's line to the end of this symbol's line,
        // which captures any leading comments between them.
        let source = indoc! {r#"
            load(
                ":a.bzl",
                "used",
                # This symbol is not needed
                "unused",
            )
            x = used()
        "#};
        let expected = indoc! {r#"
            load(
                ":a.bzl",
                "used",
            )
            x = used()
        "#};
        // The leading comment is removed along with the unused arg
        assert_eq!(run(source), expected);
    }
}
