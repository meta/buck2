/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Shared utilities for Starlark autofixes.

use std::borrow::Cow;

use ruff_python_trivia::SimpleTokenKind;
use ruff_python_trivia::SimpleTokenizer;
use ruff_source_file::LineRanges;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;

use super::parsed_module::ParsedModule;

pub(crate) fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }

    let needle_bytes = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle_bytes))
}

/// Check if the file contains a comment with the given needle (case-insensitive).
pub(crate) fn file_has_comment(module: &ParsedModule, needle: &str) -> bool {
    let range = TextRange::new(TextSize::from(0), TextSize::of(module.source()));
    module
        .comments_in_range(range)
        .any(|comment| find_ignore_ascii_case(comment, needle).is_some())
}

/// Find the start of leading comments for a statement at the given block line start.
///
/// Scans backwards from `block_line_start` to find consecutive comment lines.
/// Returns the byte offset where the leading comment block starts. If there
/// are no leading comments, returns `block_line_start`.
pub(crate) fn find_leading_comment_start(source: &str, block_line_start: usize) -> usize {
    if block_line_start == 0 {
        return 0;
    }

    let mut result = block_line_start;
    let mut pos = block_line_start;

    // Scan backwards line by line
    while pos > 0 {
        // Step back over the full line terminator first: for CRLF, `pos - 1`
        // is the `\n`, and scanning from there would find the `\r` and
        // report `pos - 1` itself as the previous line start (an empty
        // "line"), hiding a real leading comment. `LineRanges::line_start`
        // treats `\n`, `\r\n`, and lone `\r` as terminators (a raw
        // `rfind('\n')` misses lone-`\r` files entirely).
        let prev_content_end = if source[..pos].ends_with("\r\n") {
            pos - 2
        } else {
            pos - 1
        };
        let prev_line_start = line_start(source, prev_content_end);
        let prev_line = &source[prev_line_start..pos.saturating_sub(1)];
        let trimmed = prev_line.trim();

        if trimmed.starts_with('#') {
            // This is a comment line - include it
            result = prev_line_start;
            pos = prev_line_start;
        } else {
            // Non-comment line - stop scanning
            break;
        }
    }

    result
}

/// Find the end of the line containing the given byte offset.
///
/// Returns the offset of the line terminator (the `\r` of a `\r\n` pair, or
/// the `\n` / lone `\r` itself), or end of source if the line is unterminated.
/// Backed by ruff's `LineRanges` line model. Callers pairing this with
/// `offset_past_newline` advance past the full terminator, including `\r\n`.
pub(crate) fn find_line_end(source: &str, offset: TextSize) -> TextSize {
    source.line_end(offset)
}

/// Advance past the line terminator at the given offset, if present.
///
/// Skips `\r\n` as a pair as well as lone `\n` or `\r`; returns `offset`
/// unchanged otherwise. Pairs with `find_line_end` (ruff's `line_end` stops
/// *before* the terminator) to reach the start of the next line.
pub(crate) fn offset_past_newline(source: &str, offset: TextSize) -> TextSize {
    // `get` keeps this total: out-of-range or mid-char offsets (which the
    // previous byte-wise version tolerated) fall through unchanged.
    let rest = source.get(offset.to_usize()..).unwrap_or("");
    if rest.strip_prefix("\r\n").is_some() {
        offset + TextSize::from(2)
    } else if matches!(rest.as_bytes().first(), Some(b'\n') | Some(b'\r')) {
        offset + TextSize::from(1)
    } else {
        offset
    }
}

/// Find the start of the line containing the given byte offset.
///
/// Backed by ruff's `LineRanges` line model (`\n`, `\r\n`, and lone `\r` all
/// terminate lines). Returns 0 if the offset is on the first line.
pub(crate) fn line_start(source: &str, offset: usize) -> usize {
    source.line_start(TextSize::from(offset as u32)).to_usize()
}

/// Find the start of the line preceding the line that starts at `line_start_offset`.
///
/// Steps over the full line terminator first: for CRLF, `line_start_offset - 1`
/// is the `\n`, and passing that directly to [`line_start`] would find the
/// `\r` and report `line_start_offset - 1` itself as the "previous" line
/// start (an empty line). Returns 0 when there is no preceding line.
pub(crate) fn prev_line_start(source: &str, line_start_offset: usize) -> usize {
    if line_start_offset == 0 {
        return 0;
    }
    let prev_content_end = if source[..line_start_offset].ends_with("\r\n") {
        line_start_offset - 2
    } else {
        line_start_offset - 1
    };
    line_start(source, prev_content_end)
}

/// Compute line-based block ranges for a sequence of elements.
///
/// Each element's "block" spans from the line after the previous element's end
/// to the end of the current element's line, capturing leading comments and
/// trailing inline comments. The first element's block starts at `first_block_start`.
pub(crate) fn compute_block_ranges(
    source: &str,
    first_block_start: TextSize,
    element_ends: &[TextSize],
) -> Vec<TextRange> {
    element_ends
        .iter()
        .enumerate()
        .map(|(idx, &elt_end)| {
            let block_start = if idx == 0 {
                first_block_start
            } else {
                // End of the previous element's line, terminator included.
                source.full_line_end(element_ends[idx - 1])
            };

            let block_end = source.full_line_end(elt_end);

            TextRange::new(block_start, block_end)
        })
        .collect()
}

/// Validate that all block ranges fall within the source text bounds.
pub(crate) fn block_ranges_valid(ranges: &[TextRange], source: &str) -> bool {
    let bounds = TextRange::up_to(TextSize::of(source));
    ranges.iter().all(|range| bounds.contains_range(*range))
}

/// Return the source text of `block_range` with a comma guaranteed after the
/// element/value whose source ends at `elt_end`.
///
/// Lists, dicts, and call argument lists may legally omit the trailing comma on
/// their final element. Every sorting pass that reorders such elements must run
/// this before placing a block ahead of a sibling — otherwise the comma-less
/// element splices into the next one (e.g. `"a": None` directly followed by
/// `"z": 1`), producing source that fails to re-parse.
///
/// Whether a comma already follows the element is decided with ruff's
/// `SimpleTokenizer` — the same probe ruff's own formatter uses for magic
/// trailing commas — which skips whitespace, comments, newlines, and closing
/// parens. That avoids the false positives a raw byte scan invites (a comma
/// inside a string or comment, or one sitting past a line break).
///
/// The result borrows from `source` unchanged whenever a comma is already
/// present, so a fresh `String` is allocated only when one must be inserted.
/// Returns `None` when `block_range`/`elt_end` are inconsistent (the caller
/// should fall back to a safer strategy); the offsets come from AST node ranges,
/// so in practice this only guards against misuse.
pub(crate) fn block_with_trailing_comma(
    source: &str,
    block_range: TextRange,
    elt_end: TextSize,
) -> Option<Cow<'_, str>> {
    let block = source.get(block_range.start().to_usize()..block_range.end().to_usize())?;
    let elt_end_in_block = elt_end
        .to_usize()
        .checked_sub(block_range.start().to_usize())?;
    if elt_end_in_block > block.len() {
        return None;
    }
    if element_is_comma_terminated(source, elt_end, block_range.end()) {
        return Some(Cow::Borrowed(block));
    }
    let mut with_comma = String::with_capacity(block.len() + 1);
    with_comma.push_str(&block[..elt_end_in_block]);
    with_comma.push(',');
    with_comma.push_str(&block[elt_end_in_block..]);
    Some(Cow::Owned(with_comma))
}

/// True if the next significant token after `elt_end`, scanning only up to
/// `block_end`, is a comma. Mirrors ruff's `has_trailing_comma`: closing parens
/// are skipped (an element's AST range can end before the parens that wrap it),
/// and trivia — whitespace, comments, newlines — never count.
///
/// The scan is bounded to the element's own block so it cannot escape into an
/// enclosing construct: for the final element of a nested collection there is no
/// trailing comma, and an unbounded scan would skip the closing `)` and latch
/// onto an outer separator, wrongly reporting the element as comma-terminated.
fn element_is_comma_terminated(source: &str, elt_end: TextSize, block_end: TextSize) -> bool {
    SimpleTokenizer::new(source, TextRange::new(elt_end, block_end))
        .skip_trivia()
        .find(|token| token.kind() != SimpleTokenKind::RParen)
        .is_some_and(|token| token.kind() == SimpleTokenKind::Comma)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_helpers_handle_crlf_and_lone_cr() {
        // `\n`, `\r\n`, and lone `\r` all terminate lines in ruff's model.
        // Each case tabulates the expected end of the first line alongside
        // the second line start, so the assertions stay correct if a
        // fixture is edited instead of silently weakening.
        for (source, second_line_start, first_line_end) in
            [("a\nb\n", 2, 1), ("a\r\nb\r\n", 3, 1), ("a\rb\r", 2, 1)]
        {
            assert_eq!(
                line_start(source, second_line_start),
                second_line_start,
                "line_start in {source:?}"
            );
            assert_eq!(
                prev_line_start(source, second_line_start),
                0,
                "prev_line_start in {source:?}"
            );
            assert_eq!(
                find_line_end(source, TextSize::from(0)),
                TextSize::from(first_line_end),
                "find_line_end in {source:?}"
            );
            assert_eq!(
                offset_past_newline(source, find_line_end(source, TextSize::from(0))),
                TextSize::from(second_line_start as u32),
                "offset_past_newline in {source:?}"
            );
        }
    }

    #[test]
    fn test_find_leading_comment_start_sees_through_crlf() {
        // The backward scan must step over the full terminator: for CRLF,
        // scanning from `pos - 1` (the `\n`) would find the `\r` and report
        // an empty previous line, hiding the comment.
        for (source, block_start) in [
            ("# c\nload(...)", 4),
            ("# c\r\nload(...)", 5),
            ("# c\rload(...)", 4),
        ] {
            assert_eq!(
                find_leading_comment_start(source, block_start),
                0,
                "leading comment in {source:?}"
            );
        }
    }

    /// Run `block_with_trailing_comma` over the whole of `source` as the block,
    /// with the element ending at byte offset `elt_end`.
    fn comma(source: &str, elt_end: u32) -> Option<Cow<'_, str>> {
        block_with_trailing_comma(
            source,
            TextRange::new(TextSize::from(0), TextSize::of(source)),
            TextSize::from(elt_end),
        )
    }

    #[test]
    fn test_inserts_comma_when_missing() {
        // `    "a": None\n` — value ends after `None`, 13 bytes in.
        let result = comma("    \"a\": None\n", 13).expect("offset is in-bounds");
        assert!(matches!(result, Cow::Owned(_)), "must allocate to insert");
        assert_eq!(result, "    \"a\": None,\n");
    }

    #[test]
    fn test_borrows_unchanged_when_comma_present() {
        let result = comma("    \"a\": None,\n", 13).expect("offset is in-bounds");
        assert!(
            matches!(result, Cow::Borrowed(_)),
            "must not allocate when a comma is already present"
        );
        assert_eq!(result, "    \"a\": None,\n");
    }

    #[test]
    fn test_inserts_comma_before_trailing_inline_comment() {
        // The comment is trivia; with no real comma the helper inserts one right
        // after the value, before the comment.
        let result = comma("    \"a\": None  # note\n", 13).expect("offset is in-bounds");
        assert_eq!(result, "    \"a\": None,  # note\n");
    }

    #[test]
    fn test_comma_inside_comment_is_not_mistaken_for_a_trailing_comma() {
        // A byte scan that merely looks for `,` could be fooled by the comma
        // inside the comment; the tokenizer treats the whole comment as trivia.
        let result = comma("    \"a\": None  # a, b\n", 13).expect("offset is in-bounds");
        assert_eq!(result, "    \"a\": None,  # a, b\n");
    }

    #[test]
    fn test_detects_comma_past_a_newline() {
        // The comma sits on the next line. A scan that stopped at the first
        // newline would wrongly insert a second comma; the tokenizer finds it.
        let result = comma("    \"a\": None\n    ,\n", 13).expect("offset is in-bounds");
        assert!(
            matches!(result, Cow::Borrowed(_)),
            "a comma after a line break is still a trailing comma"
        );
    }

    #[test]
    fn test_honors_nonzero_block_range_start() {
        // The block is a sub-slice that does not start at offset 0.
        let source = "prefix =\n\"z\": 1\n";
        let block_range = TextRange::new(TextSize::from(9), TextSize::of(source));
        // `"z": 1` value ends after `1`, at offset 15.
        let result = block_with_trailing_comma(source, block_range, TextSize::from(15))
            .expect("offset is in-bounds");
        assert_eq!(result, "\"z\": 1,\n");
    }

    #[test]
    fn test_element_end_exactly_at_block_end() {
        // No trailing newline: the value ends at the block's final byte. The
        // comma is appended at the very end.
        let result = comma("    \"a\": None", 13).expect("offset is in-bounds");
        assert_eq!(result, "    \"a\": None,");
    }

    #[test]
    fn test_multibyte_content_before_element_end_does_not_panic() {
        // A non-ASCII char sits inside the block before `elt_end`. Slicing must
        // land on a char boundary (AST offsets always do), never mid-codepoint.
        // `"café"` is 7 bytes (é is 2), so the value ends at byte offset 7.
        let result = comma("\"café\"\n", 7).expect("offset is in-bounds");
        assert_eq!(result, "\"café\",\n");
    }

    #[test]
    fn test_returns_none_when_offset_out_of_block() {
        assert!(
            comma("\"z\": 1\n", 999).is_none(),
            "out-of-bounds element end signals the caller to fall back"
        );
    }
}
