/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context as _;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;

use super::super::parsed_module::ParsedModule;
use super::super::parsed_module::format_location;
use super::super::utils::line_start;
use super::super::utils::prev_line_start;
use crate::sort_key::SortKey;

const INLINE_SORT_KEY_PREFIX: &str = "# starlark-fmt: sort-by = ";

pub(super) struct InlineSortKey<'a> {
    json: &'a str,
    directive_range: TextRange,
}

impl InlineSortKey<'_> {
    pub(super) fn parse(&self, source: &str) -> anyhow::Result<SortKey> {
        SortKey::from_json(self.json).with_context(|| {
            format!(
                "{}: invalid `starlark-fmt: sort-by` directive",
                format_location(source, self.directive_range)
            )
        })
    }
}

pub(super) fn has_inline_sort_key(module: &ParsedModule) -> bool {
    module
        .comments_in_range(TextRange::new(
            TextSize::from(0),
            TextSize::of(module.source()),
        ))
        .any(|comment| comment.trim().starts_with(INLINE_SORT_KEY_PREFIX))
}

/// Find a `sort-by` directive above `keyword_start` that cannot attach.
///
/// Walks upward past blank and comment lines and returns the first
/// prefix-matching directive before hitting code. Call only when
/// [`inline_sort_key_before`] returned `None`: anything found here is
/// separated from the keyword (blank line, intervening comment) and would
/// otherwise be silently ignored. Directives above non-keyword statements
/// are never examined, so they no-op instead of failing the whole file.
pub(super) fn unattached_inline_directive_above(
    module: &ParsedModule,
    keyword_start: TextSize,
) -> Option<TextRange> {
    // Only a first-on-line keyword can carry an attached directive; a nested
    // same-line keyword (e.g. `name=` inside `items = [f(name = ...)`) must
    // never claim a directive that belongs to an outer keyword.
    let mut line_start_offset = keyword_line_start_if_first_on_line(module, keyword_start)?;
    let source = module.source();
    loop {
        if line_start_offset == 0 {
            return None;
        }
        let prev_start = prev_line_start(source, line_start_offset);
        let trimmed = source[prev_start..line_start_offset].trim();
        if trimmed.is_empty() {
            line_start_offset = prev_start;
            continue;
        }
        if !trimmed.starts_with('#') {
            return None;
        }
        if trimmed.starts_with(INLINE_SORT_KEY_PREFIX) {
            return Some(TextRange::new(
                TextSize::from(prev_start as u32),
                TextSize::from(line_start_offset as u32),
            ));
        }
        line_start_offset = prev_start;
    }
}

/// Shared preamble for keyword-adjacent comment lookups.
///
/// Returns the start of the keyword's line when the keyword is the first
/// non-whitespace content on its line, `None` otherwise (keyword on line 0
/// or preceded by code on the same line).
fn keyword_line_start_if_first_on_line(
    module: &ParsedModule,
    keyword_start: TextSize,
) -> Option<usize> {
    let source = module.source();
    let keyword_line_start = line_start(source, keyword_start.to_usize());
    if keyword_line_start == 0
        || !source[keyword_line_start..keyword_start.to_usize()]
            .trim()
            .is_empty()
    {
        return None;
    }
    Some(keyword_line_start)
}

pub(super) fn leading_comments_before(
    module: &ParsedModule,
    keyword_start: TextSize,
) -> Option<TextRange> {
    let source = module.source();
    let keyword_line_start = keyword_line_start_if_first_on_line(module, keyword_start)?;

    let mut leading_start = keyword_line_start;
    while leading_start > 0 {
        let candidate_start = prev_line_start(source, leading_start);
        if !source[candidate_start..leading_start]
            .trim_start()
            .starts_with('#')
        {
            break;
        }
        leading_start = candidate_start;
    }

    (leading_start < keyword_line_start).then(|| {
        TextRange::new(
            TextSize::from(leading_start as u32),
            TextSize::from(keyword_line_start as u32),
        )
    })
}

pub(super) fn inline_sort_key_before<'a>(
    module: &'a ParsedModule,
    keyword_start: TextSize,
) -> Option<InlineSortKey<'a>> {
    let source = module.source();
    let keyword_line_start = keyword_line_start_if_first_on_line(module, keyword_start)?;

    let previous_line_start = prev_line_start(source, keyword_line_start);
    let directive_range = TextRange::new(
        TextSize::from(previous_line_start as u32),
        TextSize::from(keyword_line_start as u32),
    );
    let Some(json) = source[directive_range]
        .trim()
        .strip_prefix(INLINE_SORT_KEY_PREFIX)
    else {
        return None;
    };

    Some(InlineSortKey {
        json,
        directive_range,
    })
}
