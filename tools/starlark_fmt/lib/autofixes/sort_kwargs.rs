/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Sorts keyword arguments in function calls by priority.
//!
//! Keyword arguments are sorted by their priority as defined in the config.
//! Arguments with lower priority values appear first. Arguments not in the
//! priority map sort alphabetically after prioritized arguments.
//!
//! Positional arguments are preserved in their original positions.
//!
//! ```starlark
//! # Before:
//! rust_library(
//!     deps = [":foo"],
//!     name = "bar",
//!     visibility = ["PUBLIC"],
//! )
//!
//! # After (with name=-99, deps=10, visibility=90):
//! rust_library(
//!     name = "bar",
//!     deps = [":foo"],
//!     visibility = ["PUBLIC"],
//! )
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

use super::parsed_module::Edit;
use super::parsed_module::ParsedModule;
use super::utils::block_with_trailing_comma;
use super::utils::find_line_end;
use super::utils::offset_past_newline;
use crate::config::Config;

/// Get the sort key for a kwarg name: (priority, name).
/// Args with explicit priority sort by priority first, then alphabetically.
/// Args without priority get 0 (matching buildifier's Go map behavior)
/// and sort alphabetically among themselves.
fn kwarg_sort_key<'a>(config: &Config, name: &'a str) -> (i32, &'a str) {
    let priority = config.name_priority().get(name).copied().unwrap_or(0);
    (priority, name)
}

/// Visitor that collects edits for sorting kwargs in function calls.
struct KwargSorter<'a, 'c> {
    source: &'a str,
    config: &'c Config,
    edits: Vec<Edit>,
}

impl<'a, 'c> KwargSorter<'a, 'c> {
    fn new(source: &'a str, config: &'c Config) -> Self {
        Self {
            source,
            config,
            edits: Vec::new(),
        }
    }

    /// Process a function call and sort its keyword arguments.
    ///
    /// Computes "full block" ranges for each kwarg that include leading comments
    /// and trailing inline comments/comma. When kwargs are reordered, the entire
    /// block moves together so comments stay attached to their kwarg.
    fn process_call(&mut self, call: &ExprCall) {
        let args = &call.arguments;

        // Collect keyword argument indices (skip **kwargs)
        let kwarg_indices: Vec<usize> = args
            .keywords
            .iter()
            .enumerate()
            .filter_map(|(i, kw)| kw.arg.as_ref().map(|_| i))
            .collect();

        if kwarg_indices.len() < 2 {
            return;
        }

        // Sort kwargs by priority
        let mut sorted_indices = kwarg_indices.clone();
        sorted_indices.sort_by(|&a, &b| {
            let name_a = args.keywords[a]
                .arg
                .as_ref()
                .map(|id| id.as_str())
                .unwrap_or("");
            let name_b = args.keywords[b]
                .arg
                .as_ref()
                .map(|id| id.as_str())
                .unwrap_or("");
            kwarg_sort_key(self.config, name_a).cmp(&kwarg_sort_key(self.config, name_b))
        });

        // Check if already sorted
        let already_sorted = sorted_indices
            .iter()
            .zip(kwarg_indices.iter())
            .all(|(a, b)| a == b);
        if already_sorted {
            return;
        }

        // Compute full block ranges for each kwarg.
        // A kwarg's block includes leading comments and trailing inline
        // comments. Leading content starts after the previous kwarg's
        // trailing comma (or after the opening paren for the first kwarg).
        //
        // For the FIRST kwarg in kwarg_indices:
        //   leading_start = end of last positional arg's comma, or after `(`
        // For subsequent kwargs:
        //   leading_start = after the previous kwarg's trailing comma
        //
        // trailing_end = end of line containing this kwarg's value end
        //   (includes comma and inline comment)
        //
        // If kwargs are on a single line, fall back to AST-only swapping.

        // Use single-line (AST-range) swapping when the first kwarg starts on
        // the same line as the opening paren. The multi-line block-range path
        // only works when each kwarg begins on its own line. AST-range swapping
        // handles multiline values correctly (copies the full kwarg range).
        //
        // Use call.func.range().end() (end of the function name, right before `(`)
        // rather than call.range().start() — for method calls on multiline expressions
        // like `"""...""".format(kwargs)`, call.range().start() points to the beginning
        // of the callee, not the paren.
        let first_kw = &args.keywords[kwarg_indices[0]];
        let func_end = call.func.range().end();
        let paren_line_end = find_line_end(self.source, func_end);
        let is_single_line = first_kw.range().start() <= paren_line_end;

        if is_single_line {
            // Single-line call: swap just the AST ranges (no comments to worry about)
            for (new_pos, &old_idx) in sorted_indices.iter().enumerate() {
                let target_idx = kwarg_indices[new_pos];
                if old_idx != target_idx {
                    let target_kw = &args.keywords[target_idx];
                    let source_kw = &args.keywords[old_idx];
                    let source_text = &self.source[source_kw.range()];
                    self.edits
                        .push(Edit::new(target_kw.range(), source_text.to_owned()));
                }
            }
            return;
        }

        // Multi-line call: compute full block ranges
        let block_ranges: Vec<TextRange> = kwarg_indices
            .iter()
            .enumerate()
            .map(|(pos_in_kwargs, &kw_idx)| {
                let kw = &args.keywords[kw_idx];

                // Leading start: after previous item's line end
                let leading_start = if pos_in_kwargs == 0 {
                    // First kwarg: look for positional args that appear BEFORE
                    // it (not *args which may appear after kwargs in source).
                    let last_pos_before = args
                        .args
                        .iter()
                        .filter(|a| a.range().end() <= kw.range().start())
                        .last();
                    let candidate = if let Some(last_pos_arg) = last_pos_before {
                        // After last positional arg's line end
                        let line_end = find_line_end(self.source, last_pos_arg.range().end());
                        offset_past_newline(self.source, line_end)
                    } else {
                        // No positional args: after opening paren's line end
                        let paren_line_end = find_line_end(self.source, func_end);
                        offset_past_newline(self.source, paren_line_end)
                    };

                    // If the first kwarg is on the same line as the opening paren
                    // (e.g., `func(arg1 = ..., arg2 = ...)`), the computed start
                    // would be PAST the kwarg. In that case, use the kwarg's start.
                    if candidate > kw.range().start() {
                        kw.range().start()
                    } else {
                        candidate
                    }
                } else {
                    // After previous kwarg's line end
                    let prev_kw_idx = kwarg_indices[pos_in_kwargs - 1];
                    let prev_kw = &args.keywords[prev_kw_idx];
                    let prev_line_end = find_line_end(self.source, prev_kw.range().end());
                    let candidate = offset_past_newline(self.source, prev_line_end);

                    // Same guard: if previous kwarg's value spans multiple lines
                    // or shares a line, the computed start may overshoot.
                    if candidate > kw.range().start() {
                        kw.range().start()
                    } else {
                        candidate
                    }
                };

                // Trailing end: end of the line containing this kwarg's value end
                let trailing_end = find_line_end(self.source, kw.range().end());
                // Include the newline character
                let trailing_end = offset_past_newline(self.source, trailing_end);

                TextRange::new(leading_start, trailing_end)
            })
            .collect();

        // Validate: ensure block ranges don't extend past the source
        // and don't overlap with each other. Overlapping ranges happen when
        // multiple kwargs share a line (e.g., `foo(a = 1, b = [\n...`)).
        let source_len = TextSize::of(self.source);
        let mut ranges_invalid = false;
        for range in &block_ranges {
            if range.start() > source_len || range.end() > source_len {
                ranges_invalid = true;
                break;
            }
        }
        if !ranges_invalid {
            for pair in block_ranges.windows(2) {
                if pair[0].end() > pair[1].start() {
                    ranges_invalid = true;
                    break;
                }
            }
        }
        if ranges_invalid {
            // Safety: if ranges are invalid, fall back to AST-only swapping
            for (new_pos, &old_idx) in sorted_indices.iter().enumerate() {
                let target_idx = kwarg_indices[new_pos];
                if old_idx != target_idx {
                    let target_kw = &args.keywords[target_idx];
                    let source_kw = &args.keywords[old_idx];
                    let source_text = &self.source[source_kw.range()];
                    self.edits
                        .push(Edit::new(target_kw.range(), source_text.to_owned()));
                }
            }
            return;
        }

        // Generate edits using full block ranges.
        // block_ranges[i] corresponds to kwarg_indices[i], so we need to map
        // from kwarg index to position in the block_ranges array.
        let last_kwarg_pos = kwarg_indices.len() - 1;
        for (new_pos, &old_idx) in sorted_indices.iter().enumerate() {
            let target_idx = kwarg_indices[new_pos];
            if old_idx != target_idx {
                // target_pos is always new_pos (kwarg_indices[new_pos] == target_idx)
                let target_range = block_ranges[new_pos];
                let old_pos = kwarg_indices
                    .iter()
                    .position(|&i| i == old_idx)
                    .expect("sorted_indices is a permutation of kwarg_indices");
                let source_range = block_ranges[old_pos];

                // Each kwarg becomes its own `Edit`, whose replacement is an
                // owned `String`. The final kwarg may legally omit its trailing
                // comma; when it moves to a non-final slot, insert one so it
                // doesn't splice into the following kwarg and break re-parsing.
                // A `None` return means the kwarg end fell outside its block, so
                // move it as-is.
                let source_text = if new_pos < last_kwarg_pos {
                    let moved_kw = &args.keywords[old_idx];
                    block_with_trailing_comma(self.source, source_range, moved_kw.range().end())
                        .map_or_else(|| self.source[source_range].to_owned(), |c| c.into_owned())
                } else {
                    self.source[source_range].to_owned()
                };
                self.edits.push(Edit::new(target_range, source_text));
            }
        }
    }
}

impl<'a, 'c> Visitor<'a> for KwargSorter<'a, 'c> {
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

/// Collect edits that sort keyword arguments according to config.
pub(crate) fn collect_edits(module: &ParsedModule, config: &Config) -> Vec<Edit> {
    let source = module.source();
    if source.is_empty() {
        return Vec::new();
    }

    let mut sorter = KwargSorter::new(source, config);
    for stmt in module.stmts() {
        sorter.visit_stmt(stmt);
    }

    sorter.edits
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::collections::HashSet;

    use indoc::indoc;

    use super::*;

    fn test_config() -> Config {
        let mut priority = HashMap::new();
        priority.insert("name".to_owned(), -99);
        priority.insert("gwslog_name".to_owned(), -98);
        priority.insert("deps".to_owned(), 10);
        priority.insert("exported_deps".to_owned(), 11);
        priority.insert("srcs".to_owned(), 20);
        priority.insert("src".to_owned(), 21);
        priority.insert("headers".to_owned(), 30);
        priority.insert("exported_headers".to_owned(), 31);
        priority.insert("visibility".to_owned(), 90);
        priority.insert("labels".to_owned(), 91);
        priority.insert("licenses".to_owned(), 92);

        Config::from_tables(HashSet::new(), HashSet::new(), priority)
    }

    fn run(source: &str) -> String {
        run_with_config(source, &test_config())
    }

    fn run_with_config(source: &str, config: &Config) -> String {
        ParsedModule::parse(Cow::Borrowed(source))
            .and_then(|module| module.run_transform(|m| collect_edits(m, config)))
            .expect("failed")
            .unparse()
    }

    #[test]
    fn test_sort_kwargs_by_priority() {
        let source = r#"my_rule(deps=[":foo"], name="bar", visibility=["PUBLIC"])"#;
        assert_eq!(
            run(source),
            r#"my_rule(name="bar", deps=[":foo"], visibility=["PUBLIC"])"#
        );
    }

    #[test]
    fn test_unprioritized_args_sort_alphabetically_at_priority_zero() {
        // Unprioritized args get priority 0, so they sort after negative priorities
        // but before positive priorities, alphabetically among themselves
        let source = r#"my_rule(name="x", zebra=1, alpha=2)"#;
        // name has priority -99, alpha and zebra have priority 0
        // So: name first, then alpha < zebra alphabetically
        assert_eq!(run(source), r#"my_rule(name="x", alpha=2, zebra=1)"#);
    }

    #[test]
    fn test_positional_args_preserved() {
        // Positional args stay in place, kwargs get sorted
        let source = r#"my_rule("positional", deps=[], name="x")"#;
        assert_eq!(run(source), r#"my_rule("positional", name="x", deps=[])"#);
    }

    #[test]
    fn test_mixed_positional_and_kwargs() {
        let source = r#"call(1, 2, c=3, a=1, b=2)"#;
        // Positional args (1, 2) stay, kwargs sort alphabetically (no priority)
        assert_eq!(run(source), r#"call(1, 2, a=1, b=2, c=3)"#);
    }

    #[test]
    fn test_already_sorted() {
        let source = r#"my_rule(name="x", deps=[], visibility=[])"#;
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_single_kwarg() {
        let source = r#"my_rule(name="x")"#;
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_no_kwargs() {
        let source = r#"my_rule("a", "b", "c")"#;
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_nested_calls() {
        let source = r#"outer(inner(b=2, a=1), z=3, a=4)"#;
        // Both calls get sorted independently
        assert_eq!(run(source), r#"outer(inner(a=1, b=2), a=4, z=3)"#);
    }

    #[test]
    fn test_empty_priority_sorts_alphabetically() {
        let config = Config::from_tables(HashSet::new(), HashSet::new(), HashMap::new());
        let source = r#"my_rule(z=1, a=2, m=3)"#;
        assert_eq!(
            run_with_config(source, &config),
            r#"my_rule(a=2, m=3, z=1)"#
        );
    }

    #[test]
    fn test_kwarg_sort_key() {
        let config = test_config();
        assert_eq!(kwarg_sort_key(&config, "name"), (-99, "name"));
        assert_eq!(kwarg_sort_key(&config, "deps"), (10, "deps"));
        // Unknown args get priority 0 (matching buildifier's Go map behavior)
        assert_eq!(kwarg_sort_key(&config, "unknown"), (0, "unknown"));

        // Verify ordering: name (-99) < unknown (0) < deps (10)
        let mut keys = vec!["deps", "name", "unknown", "zebra"];
        keys.sort_by(|a, b| kwarg_sort_key(&config, a).cmp(&kwarg_sort_key(&config, b)));
        assert_eq!(keys, vec!["name", "unknown", "zebra", "deps"]);
    }

    // Tests for multiline kwargs - comments stay in position, kwarg values swap

    #[test]
    fn test_kwarg_multiline_with_comments() {
        // Comments should move with their kwargs during sorting
        let source = r#"my_rule(
    # deps comment
    deps=[":foo"],
    # name comment
    name="bar",
)"#;
        // Comments move with their kwargs
        let expected = r#"my_rule(
    # name comment
    name="bar",
    # deps comment
    deps=[":foo"],
)"#;
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_kwarg_preserves_formatting() {
        let source = r#"my_rule(
    deps = [
        ":a",
        ":b",
    ],
    name = "test",
)"#;
        let expected = r#"my_rule(
    name = "test",
    deps = [
        ":a",
        ":b",
    ],
)"#;
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_nested_calls_with_kwarg_reorder() {
        // When an outer call reorders a kwarg whose value contains an inner call,
        // the inner call's kwargs must also be sorted. This tests that the
        // multi-pass loop in apply_autofixes handles this correctly.
        //
        // However, within a SINGLE sort_kwargs pass, inner edits can be
        // overwritten by outer edits (they target overlapping ranges).
        // This test verifies the single-pass behavior.
        let source = r#"outer(
    zebra = inner(c = 3, a = 1, b = 2),
    alpha = 1,
)"#;
        let result = run(source);
        // The outer call sorts: alpha before zebra
        // The inner call's kwargs may or may not be sorted in a single pass
        // depending on whether the outer edit overwrites the inner edits.
        // Either way, the outer sort must happen:
        assert!(
            result.contains("alpha = 1"),
            "alpha kwarg must appear in output"
        );
        assert!(
            result.find("alpha").expect("alpha must exist in output")
                < result.find("zebra").expect("zebra must exist in output"),
            "alpha must sort before zebra"
        );
    }

    #[test]
    fn test_inner_last_kwarg_without_comma_gets_comma_when_reordered() {
        // Regression: the inner call's last kwarg (`a = 1`) omits its trailing
        // comma and `inner(...)` is followed by an outer comma. Detecting the
        // trailing comma must be bounded to the kwarg's own block — an unbounded
        // scan would skip the inner `)` and latch onto the outer comma, wrongly
        // concluding `a = 1` is already comma-terminated and splicing it into the
        // following kwarg.
        let source = indoc! {r#"
            inner(
                b = 2,
                a = 1
            )
        "#};
        let expected = indoc! {r#"
            inner(
                a = 1,
                b = 2,
            )
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_method_call_on_multiline_string() {
        // Regression: sort_kwargs must not bleed kwargs into a triple-quoted
        // string when the call is a method on a multiline expression like
        // `"""...""".format(kwargs)`.
        let source = r#"x = """
hello {z_key} {a_key}
""".format(
    z_key = z_val,
    a_key = a_val,
)"#;
        let result = run(source);
        // After sorting, both kwargs must remain AFTER `.format(`
        assert!(result.contains(".format("), "format call must be preserved");
        // a_key should sort before z_key, both outside the string
        let format_pos = result.find(".format(").expect(".format( must exist");
        let a_pos = result
            .rfind("a_key = a_val")
            .expect("a_key kwarg must exist");
        let z_pos = result
            .rfind("z_key = z_val")
            .expect("z_key kwarg must exist");
        assert!(
            a_pos > format_pos && z_pos > format_pos,
            "kwargs must appear after .format(, not inside the string"
        );
        assert!(a_pos < z_pos, "a_key should sort before z_key");
    }

    #[test]
    fn test_kwarg_multiline_value_on_same_line() {
        // Kwargs start on the same line as paren but one value spans multiple
        // lines. Should still sort (AST-range swapping handles multiline values).
        let source = r#"x = foo(b = 1, a = bar(
    z,
))"#;
        let result = run(source);
        assert!(
            result.find("a =").expect("'a =' should be in result")
                < result.find("b =").expect("'b =' should be in result"),
            "a kwarg should sort before b even with multiline value"
        );
    }

    #[test]
    fn test_kwarg_multiline_value_with_priority() {
        // name has priority -99, should sort first even when another kwarg
        // has a multiline value on the same line as the opening paren.
        let source = r#"x = foo(deps = bar(
    z,
), name = "test")"#;
        let result = run(source);
        assert!(
            result.find("name =").expect("'name =' should be in result")
                < result.find("deps =").expect("'deps =' should be in result"),
            "name should sort before deps with multiline value"
        );
    }

    #[test]
    fn test_kwarg_on_same_line_as_paren_multiline_call() {
        // When the first kwarg is on the same line as the opening paren
        // but the call spans multiple lines (e.g., a kwarg value is a
        // multiline list), the block range computation must handle the
        // first kwarg's leading_start correctly — it should not overshoot
        // past the kwarg itself.
        let source = indoc! {r#"
            x = foo(providers = [1], doc = [
                "item1",
                "item2",
            ])
        "#};
        let result = run(source);
        // doc should sort before providers (alphabetically)
        assert!(
            result.find("doc").unwrap() < result.find("providers").unwrap(),
            "doc must sort before providers, got: {result}"
        );
    }

    #[test]
    fn test_multiline_last_kwarg_without_trailing_comma_gets_comma_when_moved() {
        // Regression: the final kwarg may omit its trailing comma. When sorting
        // moves it ahead of another kwarg, a comma must be inserted so the two
        // don't splice together into source that fails to re-parse.
        let source = indoc! {r#"
            my_rule(
                visibility = ["//foo:bar"],
                name = "foo"
            )
        "#};
        let expected = indoc! {r#"
            my_rule(
                name = "foo",
                visibility = ["//foo:bar"],
            )
        "#};
        assert_eq!(run(source), expected);
        // Idempotent: the inserted comma is not re-edited on a second pass.
        assert_eq!(run(expected), expected);
    }

    #[test]
    fn test_last_kwarg_value_ending_in_paren_gets_comma_after_paren() {
        // The relocated last kwarg's value ends in `)`. The comma must land
        // after the `)`, never inside it as `foo(,)`.
        let source = indoc! {r#"
            my_rule(
                visibility = ["//x"],
                name = foo()
            )
        "#};
        let expected = indoc! {r#"
            my_rule(
                name = foo(),
                visibility = ["//x"],
            )
        "#};
        assert_eq!(run(source), expected);
        assert_eq!(run(expected), expected);
    }

    #[test]
    fn test_nested_call_with_same_line_kwargs_outer_reorder() {
        // Outer call sorts its kwargs, moving a block containing an inner
        // call. The inner call's kwargs start on the same line as its
        // opening paren but the call spans multiple lines. Within a single
        // sort_kwargs pass, inner edits may be overwritten by outer edits
        // (the multi-pass loop in apply_autofixes handles this). Here we
        // verify the outer sort happens correctly.
        let source = indoc! {r#"
            x = outer(
                name = "test",
                docs = "long docs here",
                attrs = inner(providers = [1], doc = [
                    "item1",
                    "item2",
                ]),
            )
        "#};
        let result = run(source);
        // name has priority -99, stays first
        // attrs (0) should sort before docs (0) alphabetically
        assert!(
            result.find("attrs").unwrap() < result.find("docs").unwrap(),
            "attrs must sort before docs, got: {result}"
        );
    }
}
