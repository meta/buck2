/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::path::Path;

use ruff_python_ast::ModModule;
use ruff_python_ast::Stmt;
use ruff_python_parser::ParseError;
use ruff_python_parser::Parsed;
use ruff_python_parser::parse_module;
use ruff_python_trivia::CommentRanges;
use ruff_source_file::LineIndex;
use ruff_source_file::LineRanges;
use ruff_text_size::Ranged;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;
use tracing::info_span;

use super::fmt_suppression::find_fmt_off_ranges;
use super::fmt_suppression::overlaps_fmt_off_region;

/// Format a TextRange as line:column, line:col1-col2, or line1:col1-line2:col2.
///
/// Returns just the numbers, hyphens and colons without any prefix like "at".
/// Uses 1-indexed line and column numbers as displayed to users.
pub(crate) fn format_location(source: &str, range: TextRange) -> String {
    let index = LineIndex::from_source_text(source);
    let start = range.start();
    let end = range.end();
    let start_lc = index.line_column(start, source);
    let end_lc = index.line_column(end, source);

    if start == end {
        format!("{}:{}", start_lc.line, start_lc.column)
    } else if start_lc.line == end_lc.line {
        format!("{}:{}-{}", start_lc.line, start_lc.column, end_lc.column)
    } else {
        format!(
            "{}:{}-{}:{}",
            start_lc.line, start_lc.column, end_lc.line, end_lc.column
        )
    }
}

/// Filter out edits that fall within fmt:off regions.
fn filter_edits_by_fmt_off(edits: Vec<Edit>, fmt_off_ranges: &[TextRange]) -> Vec<Edit> {
    if fmt_off_ranges.is_empty() {
        edits
    } else {
        edits
            .into_iter()
            .filter(|edit| !overlaps_fmt_off_region(edit.range, fmt_off_ranges))
            .collect()
    }
}

/// A text edit: replace `range` with `replacement`.
#[derive(Debug, Clone)]
pub(crate) struct Edit {
    pub(crate) range: TextRange,
    pub(crate) replacement: String,
}

impl Edit {
    pub(crate) fn new(range: impl Ranged, replacement: impl Into<String>) -> Self {
        Self {
            range: range.range(),
            replacement: replacement.into(),
        }
    }

    pub(crate) fn delete(range: impl Ranged) -> Self {
        Self::new(range, "")
    }
}

/// Parsed module with precomputed comment ranges.
#[derive(Debug)]
pub struct ParsedModule<'a> {
    source: Cow<'a, str>,
    parsed: Parsed<ModModule>,
    comments: CommentRanges,
    line_index: LineIndex,
}

impl<'a> ParsedModule<'a> {
    /// Parse Starlark/Python source into a `ParsedModule`.
    ///
    /// Returns an error if the source contains syntax errors.
    pub(crate) fn parse(source: Cow<'a, str>) -> anyhow::Result<Self> {
        Self::parse_with_path(source, None)
    }

    /// Parse with optional path for error reporting. If path is provided,
    /// error message is prefixed with "path:line:col:" for structured output.
    pub(crate) fn parse_with_path(
        source: Cow<'a, str>,
        path: Option<&Path>,
    ) -> anyhow::Result<Self> {
        let parsed = parse_module(&source).map_err(|err: ParseError| {
            let pos = format_location(&source, err.location);
            match path {
                Some(p) => anyhow::anyhow!(
                    "{}:{}: failed to parse module: {}",
                    p.display(),
                    pos,
                    err.error()
                ),
                None => anyhow::anyhow!("{}: failed to parse module: {}", pos, err.error()),
            }
        })?;
        let comments: CommentRanges = parsed.tokens().into();
        let line_index = LineIndex::from_source_text(&source);
        Ok(Self {
            source,
            parsed,
            comments,
            line_index,
        })
    }

    pub(crate) fn stmts(&self) -> &[Stmt] {
        &self.parsed.syntax().body
    }

    /// Returns an iterator over comment text within the given range.
    pub(crate) fn comments_in_range(&self, range: TextRange) -> impl Iterator<Item = &str> {
        self.comments
            .comments_in_range(range)
            .iter()
            .map(|r| &self.source[*r])
    }

    /// Returns the line index built once at parse time.
    ///
    /// Pass this to helpers taking a `&LineIndex` (e.g.
    /// [`find_fmt_off_ranges`](super::fmt_suppression::find_fmt_off_ranges))
    /// so the line model is shared instead of rebuilt per call.
    pub(crate) fn line_index(&self) -> &LineIndex {
        &self.line_index
    }

    /// Returns the lexer-grounded comment ranges for the module.
    ///
    /// Prefer this over scanning source text for `#`: the index only contains
    /// real comments, so `#` inside string literals never false-positives.
    pub(crate) fn comment_ranges(&self) -> &CommentRanges {
        &self.comments
    }

    /// Returns the end position of the line containing `offset`.
    pub(crate) fn line_end(&self, offset: TextSize) -> TextSize {
        self.source.line_end(offset)
    }

    /// Returns the full line range containing `offset`, including the trailing newline.
    pub(crate) fn full_line_range(&self, offset: TextSize) -> TextRange {
        self.source.full_line_range(offset)
    }

    /// Returns the full source string.
    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    /// Returns the text for a given range.
    pub(crate) fn source_text(&self, range: TextRange) -> &str {
        &self.source[range]
    }

    /// Apply edits to source in reverse order to preserve offsets.
    /// Edits within `# fmt: off` regions are automatically filtered out.
    pub(crate) fn run_transform<F>(self, collect: F) -> anyhow::Result<Self>
    where
        F: FnOnce(&ParsedModule) -> Vec<Edit>,
    {
        self.run_transform_checked(collect)
            .map(|(module, _)| module)
    }

    /// Like `run_transform`, but also returns whether any edits were applied.
    ///
    /// Returns `(module, true)` if the source was modified, `(module, false)`
    /// if no edits were produced (or all were filtered by `fmt: off`).
    pub(crate) fn run_transform_checked<F>(self, collect: F) -> anyhow::Result<(Self, bool)>
    where
        F: FnOnce(&ParsedModule) -> Vec<Edit>,
    {
        let edits = collect(&self);
        if edits.is_empty() {
            return Ok((self, false));
        }

        let fmt_off_ranges = find_fmt_off_ranges(&self.source, &self.line_index);
        self.apply_edits(edits, &fmt_off_ranges)
    }

    /// Like `run_transform_checked`, but reuses suppression ranges already
    /// computed for this version of the source.
    pub(crate) fn run_transform_checked_with_fmt_off_ranges<F>(
        self,
        fmt_off_ranges: &[TextRange],
        collect: F,
    ) -> anyhow::Result<(Self, bool)>
    where
        F: FnOnce(&ParsedModule) -> Vec<Edit>,
    {
        let edits = collect(&self);
        self.apply_edits(edits, fmt_off_ranges)
    }

    fn apply_edits(
        self,
        edits: Vec<Edit>,
        fmt_off_ranges: &[TextRange],
    ) -> anyhow::Result<(Self, bool)> {
        let mut edits = filter_edits_by_fmt_off(edits, fmt_off_ranges);

        if edits.is_empty() {
            return Ok((self, false));
        }

        let mut source = self.source.into_owned();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start()));
        for edit in edits {
            source.replace_range(
                edit.range.start().to_usize()..edit.range.end().to_usize(),
                &edit.replacement,
            );
        }

        info_span!("reparse")
            .in_scope(|| Self::parse(Cow::Owned(source)))
            .map(|m| (m, true))
    }

    /// Consume the module and return the source string.
    ///
    /// If edits were applied via `run_transform`, returns the modified source.
    pub(crate) fn unparse(self) -> String {
        info_span!("unparse").in_scope(|| self.source.into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to parse source for tests, panics on failure.
    fn parse(source: &str) -> ParsedModule<'_> {
        ParsedModule::parse(Cow::Borrowed(source)).expect("parse should succeed")
    }

    #[test]
    fn test_parsed_module_parse_valid() {
        let module = parse("x = 1\n");
        assert_eq!(module.stmts().len(), 1);
    }

    #[test]
    fn test_parsed_module_parse_invalid() {
        let source = "def broken(\n";
        assert!(ParsedModule::parse(Cow::Borrowed(source)).is_err());
    }

    #[test]
    fn test_unparse_returns_source() {
        let source = "x = 1\n";
        assert_eq!(parse(source).unparse(), source);
    }

    #[test]
    fn test_run_transform_no_edits() {
        let source = "x = 1\n";
        let result = parse(source)
            .run_transform(|_| vec![])
            .expect("transform should succeed");
        assert_eq!(result.unparse(), source);
    }

    #[test]
    fn test_run_transform_no_edits_avoids_reparse() {
        let result = parse("x = 1\n")
            .run_transform(|_| vec![])
            .expect("transform should succeed");
        // If we reparsed, source would be Cow::Owned; since we didn't, it stays Borrowed
        assert!(
            matches!(result.source, Cow::Borrowed(_)),
            "no edits should skip reparsing and keep borrowed source"
        );
    }

    #[test]
    fn test_run_transform_single_edit() {
        let result = parse("x = 1\n")
            .run_transform(|_| {
                // Replace "1" with "42"
                vec![Edit::new(TextRange::new(4.into(), 5.into()), "42")]
            })
            .expect("transform should succeed");
        assert_eq!(result.unparse(), "x = 42\n");
    }

    #[test]
    fn test_run_transform_multiple_edits() {
        // Edits should apply correctly regardless of input order
        let result = parse("a = 1\nb = 2\n")
            .run_transform(|_| {
                vec![
                    Edit::new(TextRange::new(4.into(), 5.into()), "X"), // replace first "1"
                    Edit::new(TextRange::new(10.into(), 11.into()), "Y"), // replace "2"
                ]
            })
            .expect("transform should succeed");
        assert_eq!(result.unparse(), "a = X\nb = Y\n");
    }

    #[test]
    fn test_run_transform_delete() {
        let result = parse("x = 1 + 2\n")
            .run_transform(|_| {
                // Delete " + 2" (positions 5-9)
                vec![Edit::delete(TextRange::new(5.into(), 9.into()))]
            })
            .expect("transform should succeed");
        assert_eq!(result.unparse(), "x = 1\n");
    }

    #[test]
    fn test_comments_in_range() {
        let module = parse("x = 1  # comment\n");
        let comments: Vec<_> = module
            .comments_in_range(TextRange::new(0.into(), 20.into()))
            .collect();
        assert_eq!(comments, vec!["# comment"]);
    }

    #[test]
    fn test_comments_in_range_empty() {
        let module = parse("x = 1\n");
        let comments: Vec<_> = module
            .comments_in_range(TextRange::new(0.into(), 10.into()))
            .collect();
        assert!(comments.is_empty());
    }

    #[test]
    fn test_line_end() {
        let module = parse("x = 1\ny = 2\n");
        // Offset 2 is within "x = 1" (5 chars), line_end returns position at end of line content
        let end = module.line_end(TextSize::from(2));
        // line_end returns the position of the last char before newline (position 5)
        assert_eq!(end, TextSize::from(5));
    }

    #[test]
    fn test_run_transform_overlapping_edits() {
        // When edits overlap, the behavior depends on application order.
        // Since we sort by reverse start position, later edits (by position) apply first.
        // Overlapping edits can produce unexpected results - this documents current behavior.
        let result = parse("abcdef\n")
            .run_transform(|_| {
                vec![
                    Edit::new(TextRange::new(1.into(), 4.into()), "X"), // replace "bcd" with "X"
                    Edit::new(TextRange::new(2.into(), 5.into()), "Y"), // replace "cde" with "Y" (overlaps!)
                ]
            })
            .expect("transform should succeed");
        // Second edit (positions 2-5) applies first due to reverse sort, replacing "cde" -> "Y"
        // Result after first edit: "abYf\n"
        // First edit (positions 1-4) then replaces positions 1-4 in the ORIGINAL range
        // but applied to already-modified string, so "bYf" gets replaced with "X"
        // Result: "aX\n"
        assert_eq!(result.unparse(), "aX\n");
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn test_run_transform_edit_out_of_bounds() {
        // Edit range extends beyond source length - should panic
        let _result = parse("x = 1\n").run_transform(|_| {
            vec![Edit::new(
                TextRange::new(0.into(), 20.into()), // source is only 6 chars
                "replacement",
            )]
        });
    }

    #[test]
    fn test_run_transform_edit_at_end() {
        // Edit at exact end of source should work
        let result = parse("x = 1\n")
            .run_transform(|_| {
                vec![Edit::new(
                    TextRange::new(5.into(), 6.into()), // replace "\n" at end
                    ";\n",
                )]
            })
            .expect("transform should succeed");
        assert_eq!(result.unparse(), "x = 1;\n");
    }
}
