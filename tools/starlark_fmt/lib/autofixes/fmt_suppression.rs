/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Helpers for identifying and honoring `# fmt: off` regions.

use ruff_python_trivia::SuppressionKind;
use ruff_source_file::LineIndex;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;

/// Find all `# fmt: off` regions in the source code.
///
/// Each range starts at the beginning of the line containing the `# fmt: off`
/// comment and ends at the beginning of the line containing the corresponding
/// `# fmt: on` comment, or at the end of the source when the region is not
/// closed.
///
/// Line boundaries come from ruff's [`LineIndex`], the same line model the
/// parser uses, so `\n`, `\r\n`, and lone `\r` all terminate lines.
///
/// Takes a caller-provided index so the line model built once per parse can
/// be shared instead of rescanning the source on every call.
pub(super) fn find_fmt_off_ranges(source: &str, line_index: &LineIndex) -> Vec<TextRange> {
    let starts = line_index.line_starts();

    let mut ranges = Vec::new();
    let mut off_start = None;

    for (line, &line_start) in starts.iter().enumerate() {
        let line_start = line_start.to_usize();
        if line_start == source.len() {
            break;
        }
        let line_end = starts
            .get(line + 1)
            .map(|end| end.to_usize())
            .unwrap_or(source.len());
        let line_text = &source[line_start..line_end];
        // Most lines hold no comment at all; skip `trim` plus the comment
        // parser for them. Sound: a suppression marker is always a `#`
        // comment, so a `#`-less line can never match.
        if !line_text.as_bytes().contains(&b'#') {
            continue;
        }
        match SuppressionKind::from_comment(line_text.trim()) {
            Some(SuppressionKind::Off) if off_start.is_none() => {
                off_start = Some(TextSize::from(line_start as u32));
            }
            Some(SuppressionKind::On) => {
                if let Some(start) = off_start.take() {
                    ranges.push(TextRange::new(start, TextSize::from(line_start as u32)));
                }
            }
            _ => {}
        }
    }

    if let Some(start) = off_start {
        ranges.push(TextRange::new(start, TextSize::from(source.len() as u32)));
    }

    ranges
}

/// Check whether a text range overlaps any `# fmt: off` region.
///
/// Overlap is strict: a zero-width range exactly at a region boundary, or a
/// range that merely touches a boundary, does not count as overlapping. This
/// preserves the long-standing semantics from `parsed_module.rs` this helper
/// was extracted from; callers needing boundary-inclusive checks should
/// expand the range first.
pub(super) fn overlaps_fmt_off_region(range: TextRange, fmt_off_ranges: &[TextRange]) -> bool {
    fmt_off_ranges
        .iter()
        .any(|suppressed| suppressed.start() < range.end() && range.start() < suppressed.end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_fmt_off_ranges_with_crlf() {
        let source = "x = 1\r\n# fmt: off\r\ny = 2\r\n# fmt: on\r\nz = 3\r\n";
        let off = source.find("# fmt: off").unwrap() as u32;
        let on = source.find("# fmt: on").unwrap() as u32;

        assert_eq!(
            find_fmt_off_ranges(source, &LineIndex::from_source_text(source)),
            vec![TextRange::new(TextSize::from(off), TextSize::from(on))],
            "CRLF line endings should still map each comment to its line start",
        );
    }

    #[test]
    fn test_find_fmt_off_ranges_with_lone_cr() {
        let source = "x = 1\r# fmt: off\ry = 2\r# fmt: on\rz = 3\r";
        let off = source.find("# fmt: off").unwrap() as u32;
        let on = source.find("# fmt: on").unwrap() as u32;

        assert_eq!(
            find_fmt_off_ranges(source, &LineIndex::from_source_text(source)),
            vec![TextRange::new(TextSize::from(off), TextSize::from(on))],
            "lone-CR line endings should produce one line per segment",
        );
    }

    #[test]
    fn test_find_fmt_off_ranges_no_suppression() {
        let source = "x = 1\ny = 2\n";
        assert!(
            find_fmt_off_ranges(source, &LineIndex::from_source_text(source)).is_empty(),
            "source without suppression comments should yield no ranges"
        );
    }

    #[test]
    fn test_find_fmt_off_ranges_unclosed() {
        let source = "x = 1\n# fmt: off\ny = 2\n";
        let off = source.find("# fmt: off").unwrap() as u32;

        assert_eq!(
            find_fmt_off_ranges(source, &LineIndex::from_source_text(source)),
            vec![TextRange::new(
                TextSize::from(off),
                TextSize::from(source.len() as u32)
            )],
            "unclosed `# fmt: off` should extend to end of source",
        );
    }

    #[test]
    fn test_find_fmt_off_ranges_stray_on() {
        let source = "x = 1\n# fmt: on\ny = 2\n";
        assert!(
            find_fmt_off_ranges(source, &LineIndex::from_source_text(source)).is_empty(),
            "`# fmt: on` without a preceding `# fmt: off` should yield no ranges"
        );
    }

    #[test]
    fn test_overlaps_fmt_off_region_boundaries() {
        let source = "x = 1\n# fmt: off\ny = 2\n# fmt: on\nz = 3\n";
        let ranges = find_fmt_off_ranges(source, &LineIndex::from_source_text(source));
        assert_eq!(ranges.len(), 1);
        let region = ranges[0];

        assert!(
            overlaps_fmt_off_region(
                TextRange::new(
                    region.start() + TextSize::from(1),
                    region.end() - TextSize::from(1)
                ),
                &ranges
            ),
            "a range strictly inside the region should overlap"
        );
        for boundary in [region.start(), region.end()] {
            assert!(
                !overlaps_fmt_off_region(TextRange::empty(boundary), &ranges),
                "zero-width range at {boundary:?} should not overlap"
            );
        }
        assert!(
            !overlaps_fmt_off_region(TextRange::new(TextSize::from(0), region.start()), &ranges),
            "a range merely touching the region start should not overlap"
        );
        assert!(
            !overlaps_fmt_off_region(
                TextRange::new(region.end(), TextSize::from(source.len() as u32)),
                &ranges
            ),
            "a range merely touching the region end should not overlap"
        );
    }

    #[test]
    fn test_find_fmt_off_ranges_indented_comment() {
        let source = "x = 1\n    # fmt: off\ny = 2\n# fmt: on\nz = 3\n";
        let line_start = source.find("    # fmt: off").unwrap() as u32;
        let on = source.find("# fmt: on").unwrap() as u32;

        assert_eq!(
            find_fmt_off_ranges(source, &LineIndex::from_source_text(source)),
            vec![TextRange::new(
                TextSize::from(line_start),
                TextSize::from(on)
            )],
            "range should start at the beginning of the line holding `# fmt: off`",
        );
    }
}
