/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Moves load statements to the top of the file, after docstrings, comments, and
//! workspace() calls. Matches Buildifier's `loadTop` rewrite behavior.
//!
//! Load statements that appear after other statements are moved to the top,
//! preserving their relative order. This ensures loads are grouped at the top
//! of the file for consistency and readability.
//!
//! The transformation is skipped if:
//! - The file contains a `disable=load-on-top` comment anywhere
//! - The file is a WORKSPACE file (cannot be detected without filename; workspace()
//!   calls are preserved at the top as a heuristic)

use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_text_size::Ranged;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;

use super::load_utils::is_load_call;
use super::parsed_module::Edit;
use super::parsed_module::ParsedModule;
use crate::autofixes::utils::file_has_comment;
use crate::autofixes::utils::find_leading_comment_start;
use crate::autofixes::utils::line_start;

/// Check if a statement is a docstring (string literal expression).
fn is_docstring(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Expr(expr_stmt) if matches!(expr_stmt.value.as_ref(), Expr::StringLiteral(_))
    )
}

/// Check if a statement is a workspace() call.
fn is_workspace_call(stmt: &Stmt) -> bool {
    if let Stmt::Expr(expr_stmt) = stmt
        && let Expr::Call(call) = expr_stmt.value.as_ref()
        && let Expr::Name(name) = call.func.as_ref()
    {
        name.id.as_str() == "workspace"
    } else {
        false
    }
}

/// Check if a statement is a load() call.
fn is_load_stmt(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Expr(expr_stmt) if is_load_call(&expr_stmt.value))
}

/// Advance over blank (whitespace-only) lines starting at `from`, which must be
/// at the start of a line. Returns the offset past the consumed blank lines.
fn consume_blank_lines(source: &str, from: usize) -> usize {
    let mut end = from;
    while end < source.len() {
        let line_end = source[end..]
            .find('\n')
            .map_or(source.len(), |i| end + i + 1);
        if !source[end..line_end].trim().is_empty() {
            break;
        }
        end = line_end;
    }
    end
}

/// Collect edits that move load statements to the top of the file.
pub fn collect_edits(module: &ParsedModule) -> Vec<Edit> {
    let stmts = module.stmts();
    if stmts.is_empty() {
        return Vec::new();
    }

    // Skip if file contains disable=load-on-top comment anywhere
    if file_has_comment(module, "disable=load-on-top") {
        return Vec::new();
    }

    // Find the index of the first non-load statement (excluding docstrings,
    // comments, and workspace() calls which are allowed at the top)
    let mut first_stmt_index: Option<usize> = None;
    let mut misplaced_load_indices = Vec::new();

    for (idx, stmt) in stmts.iter().enumerate() {
        // Skip docstrings, workspace() calls - these are allowed at the top
        if is_docstring(stmt) || is_workspace_call(stmt) {
            continue;
        }

        if is_load_stmt(stmt) {
            // This is a load statement
            if first_stmt_index.is_some() {
                // We've seen a non-load statement already, so this load is misplaced
                misplaced_load_indices.push(idx);
            }
            // If first_stmt_index is None, this load is at the top (good)
        } else {
            // This is a non-load, non-docstring, non-workspace statement
            if first_stmt_index.is_none() {
                first_stmt_index = Some(idx);
            }
        }
    }

    if misplaced_load_indices.is_empty() {
        return Vec::new();
    }

    // For each misplaced load, capture its text (leading comments included) and
    // the range to delete: the load line plus any trailing blank lines. Consuming
    // the trailing blank keeps the body tight; one blank line is re-added after
    // the relocated loads below.
    let source = module.source();

    let (loads_to_move, ranges_to_delete) = misplaced_load_indices.iter().fold(
        (Vec::new(), Vec::new()),
        |(mut loads, mut ranges), &idx| {
            let stmt_range = stmts[idx].range();

            // Start of this load's own leading comments (if any).
            let stmt_start = stmt_range.start().to_usize();
            let line_start = line_start(source, stmt_start);
            let comment_start = find_leading_comment_start(source, line_start);

            // End of the load's line, then over any trailing blank lines.
            let line_end = module.line_end(stmt_range.end());
            let after_load = if line_end < TextSize::of(source) {
                line_end.to_usize() + 1
            } else {
                line_end.to_usize()
            };
            let delete_end = consume_blank_lines(source, after_load);

            let delete_range = TextRange::new(
                TextSize::from(comment_start as u32),
                TextSize::from(delete_end as u32),
            );
            let load_text = module.source_text(delete_range).trim_end().to_owned();

            loads.push(load_text);
            ranges.push(delete_range);
            (loads, ranges)
        },
    );

    // Insert at the start of the first non-load statement, i.e. after any leading
    // header comments, which stay at the very top.
    let insert_pos = if let Some(idx) = first_stmt_index {
        stmts[idx].range().start()
    } else {
        TextSize::of(source)
    };

    // A module docstring keeps a blank line before the relocated loads
    // (Buildifier convention); workspace() calls and header comments do not.
    let leading_blank = first_stmt_index
        .and_then(|idx| idx.checked_sub(1))
        .is_some_and(|prev| is_docstring(&stmts[prev]));

    // Delete the misplaced loads (reverse order to preserve offsets), then insert
    // them grouped at the top, followed by a single blank line separating the
    // loads from the body (Buildifier convention).
    let mut edits = ranges_to_delete
        .into_iter()
        .rev()
        .map(Edit::delete)
        .collect::<Vec<_>>();

    let mut inserted = String::new();
    if leading_blank {
        inserted.push('\n');
    }
    inserted.push_str(&loads_to_move.join("\n"));
    inserted.push_str("\n\n");

    edits.push(Edit::new(TextRange::new(insert_pos, insert_pos), inserted));

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
    fn test_move_single_load_to_top() {
        let source = indoc! {r#"
            x = 1
            load("//a:a.bzl", "a")

            y = a()
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a")

            x = 1
            y = a()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_move_multiple_loads_to_top() {
        let source = indoc! {r#"
            x = 1
            load("//a:a.bzl", "a")

            y = a()
            load("//b:b.bzl", "b")

            z = b()
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a")
            load("//b:b.bzl", "b")

            x = 1
            y = a()
            z = b()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_preserves_loads_already_at_top() {
        let source = indoc! {r#"
            load("//a:a.bzl", "a")
            load("//b:b.bzl", "b")

            x = a()
            y = b()
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_preserves_docstring_at_top() {
        let source = indoc! {r#"
            """Module docstring."""
            x = 1
            load("//a:a.bzl", "a")

            y = a()
        "#};
        let expected = indoc! {r#"
            """Module docstring."""

            load("//a:a.bzl", "a")

            x = 1
            y = a()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_preserves_workspace_call_at_top() {
        let source = indoc! {r#"
            workspace(name = "my_workspace")
            x = 1
            load("//a:a.bzl", "a")

            y = a()
        "#};
        let expected = indoc! {r#"
            workspace(name = "my_workspace")
            load("//a:a.bzl", "a")

            x = 1
            y = a()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_preserves_leading_comments() {
        let source = indoc! {r#"
            # This is a comment
            x = 1
            # Load comment
            load("//a:a.bzl", "a")

            y = a()
        "#};
        let expected = indoc! {r#"
            # This is a comment
            # Load comment
            load("//a:a.bzl", "a")

            x = 1
            y = a()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_disable_comment_skips_transformation() {
        let source = indoc! {r#"
            # buildifier: disable=load-on-top
            x = 1
            load("//a:a.bzl", "a")

            y = 2
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_disable_comment_anywhere_skips() {
        let source = indoc! {r#"
            x = 1
            load("//a:a.bzl", "a")
            # disable=load-on-top
            y = 2
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_no_loads() {
        let source = indoc! {r#"
            x = 1
            y = 2
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_only_loads() {
        let source = indoc! {r#"
            load("//a:a.bzl", "a")
            load("//b:b.bzl", "b")
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_load_with_inline_comment() {
        let source = indoc! {r#"
            x = 1
            load("//a:a.bzl", "a")  # important load
            y = 2
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a")  # important load

            x = 1
            y = 2
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_load_with_single_leading_comment() {
        // Load with a single leading comment on the preceding line - comment moves with load
        let source = indoc! {r#"
            x = 1
            # This load is important
            load("//a:a.bzl", "a")

            y = a()
        "#};
        let expected = indoc! {r#"
            # This load is important
            load("//a:a.bzl", "a")

            x = 1
            y = a()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_load_with_multiple_leading_comments() {
        // Load with multiple consecutive leading comment lines - all move together with load
        let source = indoc! {r#"
            x = 1
            # First comment line
            # Second comment line
            # Third comment line
            load("//a:a.bzl", "a")

            y = a()
        "#};
        let expected = indoc! {r#"
            # First comment line
            # Second comment line
            # Third comment line
            load("//a:a.bzl", "a")

            x = 1
            y = a()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_load_with_inline_trailing_comment() {
        // Load with inline trailing comment - comment stays attached to load after moving
        let source = indoc! {r#"
            x = 1
            load("//a:a.bzl", "a")  # trailing comment here
            y = a()
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a")  # trailing comment here

            x = 1
            y = a()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_non_load_leading_comment_stays() {
        // Non-load statement with its own leading comment - comment stays with statement, does NOT move with load
        let source = indoc! {r#"
            # Comment for x
            x = 1
            # Comment for load
            load("//a:a.bzl", "a")

            y = a()
        "#};
        let expected = indoc! {r#"
            # Comment for x
            # Comment for load
            load("//a:a.bzl", "a")

            x = 1
            y = a()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_loads_already_sorted_noop() {
        // Confirm that if loads are already at the top (sorted), we noop
        let source = indoc! {r#"
            load("//a:a.bzl", "a")
            load("//b:b.bzl", "b")

            x = a()
            y = b()
        "#};
        // Should be unchanged - loads already at top
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_no_edits_when_loads_at_top() {
        // Prove that when loads are already at the top, collect_edits returns
        // an empty Vec, avoiding the expensive reparse step in run_transform.
        // This is a stronger guarantee than just checking source unchanged.
        let source = indoc! {r#"
            load("//a:a.bzl", "a")
            load("//b:b.bzl", "b")

            x = a()
            y = b()
        "#};
        let module = ParsedModule::parse(Cow::Borrowed(source)).expect("parse should succeed");
        let edits = collect_edits(&module);
        // Critical: edits must be empty to avoid reparse
        assert!(
            edits.is_empty(),
            "Expected no edits when loads are already at top, but got {} edits",
            edits.len()
        );
    }
}
