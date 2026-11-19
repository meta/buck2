/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Sorts load statements lexicographically by path and sorts symbols within each load.
//!
//! Load statements are sorted by their path argument (first string argument).
//! Within each load statement, symbols are sorted alphabetically, with positional
//! arguments (string literals) first, followed by keyword arguments sorted by key.
//!
//! ```starlark
//! # Before:
//! load("//z:z.bzl", "z_rule")
//! load("//a:a.bzl", "c_func", "a_func", renamed="original")
//!
//! # After:
//! load("//a:a.bzl", "a_func", "c_func", renamed="original")
//! load("//z:z.bzl", "z_rule")
//! ```

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;

use ruff_text_size::Ranged;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;

use super::load_utils::LoadStatement;
use super::load_utils::LoadSymbol;
use super::load_utils::extract_load_statements;
use super::load_utils::reconstruct_load;
use super::parsed_module::Edit;
use super::parsed_module::ParsedModule;
use super::utils::file_has_comment;
use super::utils::find_leading_comment_start;
use super::utils::line_start;
use crate::autofixes::utils::find_ignore_ascii_case;

// Sentinel package marker used so relative labels (`:foo`) sort *after* all
// real packages in `compare_load_labels`. `\u{7f}` (DEL) is the highest ASCII
// code point — it sorts after any other byte that can legally appear in a
// Bazel/Buck package path. Bazel/Buck package paths are restricted to ASCII,
// so no real package can collide with this sentinel in practice.
const RELATIVE_LABEL_PACKAGE: &str = "\u{7f}";

#[derive(Debug, Clone, Copy)]
struct ParsedLoadLabel<'a> {
    repository: &'a str,
    package: &'a str,
    target: &'a str,
}

/// Parse load labels using the parts Buildifier compares in compareLoadLabels.
fn parse_load_label(label: &str) -> ParsedLoadLabel<'_> {
    if label.starts_with(':') {
        return ParsedLoadLabel {
            repository: "",
            package: RELATIVE_LABEL_PACKAGE,
            target: label.trim_start_matches(':'),
        };
    }

    let (repository, absolute) = if let Some(rest) = label.strip_prefix('@') {
        if let Some((repository, rest)) = rest.split_once("//") {
            (repository, Some(rest))
        } else {
            (rest, Some(""))
        }
    } else if let Some(rest) = label.strip_prefix("//") {
        ("", Some(rest))
    } else {
        (label, None)
    };

    let Some(rest) = absolute else {
        return ParsedLoadLabel {
            repository: "",
            package: RELATIVE_LABEL_PACKAGE,
            target: label,
        };
    };

    let (package, target) = if let Some((package, target)) = rest.rsplit_once(':') {
        (package, target)
    } else if let Some((package, target)) = rest.rsplit_once('/') {
        (package, target)
    } else {
        ("", rest)
    };

    ParsedLoadLabel {
        repository,
        package,
        target,
    }
}

/// Compare paths the way Buildifier does: slash-delimited chunks compare
/// case-insensitively first, with a case-sensitive fallback.
fn compare_paths(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }

    let mut a_chunks = a.split('/');
    let mut b_chunks = b.split('/');

    loop {
        match (a_chunks.next(), b_chunks.next()) {
            (Some(a_chunk), Some(b_chunk)) => {
                // Compare lowercase bytes lazily without allocating a `String`
                // per chunk. ASCII-only is sufficient since Bazel/Buck labels
                // never contain non-ASCII characters; this matches the
                // original `to_ascii_lowercase()` behavior byte-for-byte.
                let ord = a_chunk
                    .bytes()
                    .map(|byte| byte.to_ascii_lowercase())
                    .cmp(b_chunk.bytes().map(|byte| byte.to_ascii_lowercase()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return a.cmp(b),
        }
    }
}

fn compare_load_labels(a: &str, b: &str) -> Ordering {
    let a_has_explicit_repo = a.starts_with('@');
    let b_has_explicit_repo = b.starts_with('@');
    if a_has_explicit_repo != b_has_explicit_repo {
        // Reversed cmp: `@`-prefixed (explicit-repo) loads sort *before*
        // non-`@` loads, matching Buildifier's compareLoadLabels.
        return b_has_explicit_repo.cmp(&a_has_explicit_repo);
    }

    let a_label = parse_load_label(a);
    let b_label = parse_load_label(b);

    a_label
        .repository
        .cmp(b_label.repository)
        .then_with(
            || match (a_label.package.is_empty(), b_label.package.is_empty()) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => Ordering::Equal,
            },
        )
        .then_with(|| compare_paths(a_label.package, b_label.package))
        .then_with(|| compare_paths(a_label.target, b_label.target))
}

/// Get the full line range for the first line of a load statement.
/// For single-line loads this is the entire load; for multiline loads this is just the `load(` line.
/// Used for extracting leading comments when sorting.
fn first_line_range_for_load(load: &LoadStatement<'_>, module: &ParsedModule) -> TextRange {
    module.full_line_range(load.stmt_range.start())
}

/// Get the end position after the last line of a load statement (handles multiline loads).
/// Returns the byte offset immediately after the newline following the closing `)`.
fn load_stmt_end_of_last_line(load: &LoadStatement<'_>, source: &str) -> TextSize {
    source[load.stmt_range.end().to_usize()..]
        .find('\n')
        .map(|pos| load.stmt_range.end() + TextSize::from((pos + 1) as u32))
        .unwrap_or(TextSize::from(source.len() as u32))
}

/// Get the text after the parsed load statement on its closing line.
///
/// `stmt_range` ends before a trailing statement comment, but group sorting
/// replaces whole lines. Preserve comments like `# @unused` when the load text
/// is reconstructed for symbol sorting.
fn trailing_text_for_load<'a>(load: &LoadStatement<'_>, module: &'a ParsedModule) -> &'a str {
    let line_end = module.line_end(load.stmt_range.end());
    module
        .source_text(TextRange::new(load.stmt_range.end(), line_end))
        .trim_end()
}

/// Get the range of a load statement including any leading comments.
///
/// This captures any comment lines immediately preceding the load statement,
/// so that comments move with their associated load when reordering.
///
/// The `prev_end` parameter is the end of the previous statement or the start of the file,
/// used to determine where to start looking for leading comments.
fn load_range_with_leading_comments(
    load: &LoadStatement<'_>,
    prev_end: ruff_text_size::TextSize,
    source: &str,
) -> TextRange {
    // Include everything from prev_end (which might include leading comments) to load line end
    let load_line_end = source[load.stmt_range.end().to_usize()..]
        .find('\n')
        .map(|pos| load.stmt_range.end() + ruff_text_size::TextSize::from((pos + 1) as u32))
        .unwrap_or(ruff_text_size::TextSize::from(source.len() as u32));

    // Start from prev_end to capture leading comments
    TextRange::new(prev_end, load_line_end)
}

/// Strip leading whitespace-only lines from `text`, keeping any leading
/// comment lines intact.
///
/// Used to tighten a load block: blank lines captured between consecutive
/// loads (or before a load's leading comment) are dropped so the
/// reconstructed group has no blank lines between loads, matching Buildifier.
fn strip_leading_blank_lines(text: &str) -> &str {
    let mut rest = text;
    while let Some(newline) = rest.find('\n') {
        if rest[..newline].trim().is_empty() {
            rest = &rest[newline + 1..];
        } else {
            break;
        }
    }
    rest
}

/// Get sort key for a symbol (for sorting symbols within a load).
/// Returns (is_keyword, sort_key) where positional args come before keyword args.
fn symbol_sort_key<'a>(sym: &'a LoadSymbol<'a>) -> (bool, &'a str) {
    let is_alias = sym
        .original_name
        .is_some_and(|original_name| original_name != sym.bound_name);
    (is_alias, sym.bound_name)
}

fn sorted_deduplicated_symbols<'a>(symbols: &[LoadSymbol<'a>]) -> Vec<LoadSymbol<'a>> {
    let mut sorted = symbols.to_vec();
    sorted.sort_by(|a, b| symbol_sort_key(a).cmp(&symbol_sort_key(b)));

    // `Vec::dedup_by` keeps the first of each consecutive equal group, matching
    // Buildifier's behavior and the list-sort dedup in `sort_list_args.rs`
    // (which uses `HashSet::insert` and skips on collision). For
    // `load(":a.bzl", "foo", foo="bar")` this preserves the positional `"foo"`
    // and drops the alias.
    sorted.dedup_by(|a, b| a.bound_name == b.bound_name);
    sorted
}

fn symbols_are_sorted_and_deduplicated(symbols: &[LoadSymbol<'_>]) -> bool {
    symbols.windows(2).all(|pair| {
        symbol_sort_key(&pair[0]) <= symbol_sort_key(&pair[1])
            && pair[0].bound_name != pair[1].bound_name
    })
}

/// Check if a load statement's symbols need sorting.
fn needs_symbol_sorting(load: &LoadStatement<'_>) -> bool {
    !symbols_are_sorted_and_deduplicated(&load.symbols)
}

fn load_comment_range(load: &LoadStatement<'_>, module: &ParsedModule) -> TextRange {
    TextRange::new(
        load.stmt_range.start(),
        module.line_end(load.stmt_range.end()),
    )
}

fn load_has_comment(load: &LoadStatement<'_>, module: &ParsedModule, needle: &str) -> bool {
    module
        .comments_in_range(load_comment_range(load, module))
        .any(|comment| find_ignore_ascii_case(comment, needle).is_some())
}

/// Find contiguous groups of load statements in the module.
/// Returns a list of (start_index, end_index) pairs where each pair represents
/// a contiguous group of load statements.
fn find_load_groups(
    load_stmts: &[LoadStatement<'_>],
    module: &ParsedModule,
) -> Vec<(usize, usize)> {
    if load_stmts.is_empty() {
        return Vec::new();
    }

    let stmts = module.stmts();

    // Find the statement indices of each load statement
    let load_indices: Vec<usize> = stmts
        .iter()
        .enumerate()
        .filter(|(_, stmt)| {
            load_stmts
                .iter()
                .any(|load| load.stmt_range == stmt.range())
        })
        .map(|(idx, _)| idx)
        .collect();

    if load_indices.is_empty() {
        return Vec::new();
    }

    // Group contiguous load indices using fold to track break points
    let (mut groups, final_start) = load_indices.windows(2).enumerate().fold(
        (Vec::new(), 0),
        |(mut groups, start), (i, window)| {
            if window[1] != window[0] + 1 {
                groups.push((start, i + 1));
                (groups, i + 1)
            } else {
                (groups, start)
            }
        },
    );
    groups.push((final_start, load_indices.len()));

    groups
}

#[derive(Debug, Clone)]
struct LoadRecord<'a> {
    load: &'a LoadStatement<'a>,
    symbols: Cow<'a, [LoadSymbol<'a>]>,
    removed: bool,
    merged_symbols: bool,
}

fn load_records<'a>(
    load_stmts: &'a [LoadStatement<'a>],
    groups: &[(usize, usize)],
    module: &ParsedModule,
) -> Vec<LoadRecord<'a>> {
    let mut records: Vec<LoadRecord<'a>> = load_stmts
        .iter()
        .map(|load| LoadRecord {
            load,
            symbols: Cow::Borrowed(load.symbols.as_slice()),
            removed: false,
            merged_symbols: false,
        })
        .collect();

    // Merge same-origin loads only within each group. Cross-group merging
    // would leave orphaned load text in the output for groups whose loads
    // all get merged into a load in another group, since the outer loop
    // skips groups whose records are all `removed` without emitting a
    // deletion edit.
    for &(start, end) in groups {
        // HashMap (vs. linear-scan Vec) keeps per-group merging O(n) instead
        // of O(n^2) for groups with many distinct module paths.
        let mut first_by_module: HashMap<&str, usize> = HashMap::new();

        for idx in start..end {
            let module_path = load_stmts[idx].module_path;
            let Some(&previous_idx) = first_by_module.get(module_path) else {
                first_by_module.insert(module_path, idx);
                continue;
            };

            if load_has_comment(
                &load_stmts[previous_idx],
                module,
                "disable=same-origin-load",
            ) || load_has_comment(&load_stmts[idx], module, "disable=same-origin-load")
            {
                continue;
            }

            records[previous_idx]
                .symbols
                .to_mut()
                .extend(load_stmts[idx].symbols.iter().copied());
            records[previous_idx].merged_symbols = true;
            records[idx].removed = true;
        }
    }

    records
}

fn record_needs_reconstruction(record: &LoadRecord<'_>) -> bool {
    record.merged_symbols || !symbols_are_sorted_and_deduplicated(record.symbols.as_ref())
}

fn reconstruct_record(record: &LoadRecord<'_>) -> String {
    let symbols = sorted_deduplicated_symbols(record.symbols.as_ref());
    reconstruct_load(record.load.module_path, &symbols)
}

fn load_source_with_sorted_symbols(
    load: &LoadStatement<'_>,
    module: &ParsedModule,
) -> Option<String> {
    if load.symbols.len() < 2 {
        return None;
    }

    let source = module.source();
    let first_symbol_line_start = line_start(source, load.symbols[0].range.start().to_usize());
    let closing_line_start = line_start(source, load.stmt_range.end().to_usize());
    if first_symbol_line_start == closing_line_start {
        return None;
    }

    if symbols_are_sorted_and_deduplicated(&load.symbols) {
        return None;
    }

    let sorted_symbols = sorted_deduplicated_symbols(&load.symbols);

    let prefix = &source[load.stmt_range.start().to_usize()..first_symbol_line_start];
    let suffix = &source[closing_line_start..load.stmt_range.end().to_usize()];

    let symbol_blocks: Vec<(usize, &str)> = load
        .symbols
        .iter()
        .enumerate()
        .map(|(idx, symbol)| {
            let start = line_start(source, symbol.range.start().to_usize());
            let end = load
                .symbols
                .get(idx + 1)
                .map_or(closing_line_start, |next_symbol| {
                    line_start(source, next_symbol.range.start().to_usize())
                });
            (start, &source[start..end])
        })
        .collect();

    let mut result = String::from(prefix);
    for (sorted_idx, symbol) in sorted_symbols.iter().enumerate() {
        let original_idx = load
            .symbols
            .iter()
            .position(|original| original.range == symbol.range)?;
        let (block_start, block) = symbol_blocks[original_idx];
        if sorted_idx + 1 == sorted_symbols.len() || symbol_has_trailing_comma(source, symbol) {
            result.push_str(block);
        } else {
            result.push_str(&symbol_block_with_inserted_comma(
                block_start,
                block,
                symbol,
            ));
        }
    }
    result.push_str(suffix);
    Some(result)
}

fn symbol_has_trailing_comma(source: &str, symbol: &LoadSymbol<'_>) -> bool {
    source[symbol.range.end().to_usize()..]
        .trim_start()
        .starts_with(',')
}

fn symbol_block_with_inserted_comma(
    block_start: usize,
    block: &str,
    symbol: &LoadSymbol<'_>,
) -> String {
    let insertion_offset = symbol.range.end().to_usize() - block_start;
    let mut result = String::with_capacity(block.len() + 1);
    result.push_str(&block[..insertion_offset]);
    result.push(',');
    result.push_str(&block[insertion_offset..]);
    result
}

fn render_record(record: &LoadRecord<'_>, module: &ParsedModule) -> String {
    if !record.merged_symbols {
        if let Some(source) = load_source_with_sorted_symbols(record.load, module) {
            return source;
        }
    }

    reconstruct_record(record)
}

/// Collect edits that sort load statements and their symbols.
pub fn collect_edits(module: &ParsedModule) -> Vec<Edit> {
    let stmts = module.stmts();
    if stmts.is_empty() {
        return Vec::new();
    }

    let load_stmts = extract_load_statements(stmts);
    if load_stmts.is_empty() {
        return Vec::new();
    }

    if file_has_comment(module, "disable=out-of-order-load") {
        return load_stmts
            .iter()
            .filter(|load| needs_symbol_sorting(load))
            .map(|load| {
                let record = LoadRecord {
                    load,
                    symbols: Cow::Borrowed(load.symbols.as_slice()),
                    removed: false,
                    merged_symbols: false,
                };
                Edit::new(load.stmt_range, render_record(&record, module))
            })
            .collect();
    }

    // Find contiguous groups of load statements first; same-origin merging
    // is restricted to within these groups to avoid leaving orphaned loads
    // when a non-load statement separates two loads from the same module.
    let groups = find_load_groups(&load_stmts, module);
    let records = load_records(&load_stmts, &groups, module);
    let mut edits: Vec<Edit> = Vec::new();

    for (start, end) in groups {
        let group: Vec<_> = records[start..end]
            .iter()
            .filter(|record| !record.removed)
            .collect();
        if group.is_empty() {
            continue;
        }

        // Check if the group is already sorted
        let mut sorted_indices: Vec<usize> = (0..group.len()).collect();
        sorted_indices.sort_by(|&a, &b| {
            let load_a = group[a];
            let load_b = group[b];
            compare_load_labels(load_a.load.module_path, load_b.load.module_path)
        });

        let is_sorted = sorted_indices.iter().enumerate().all(|(i, &j)| i == j);
        let has_removed_loads = records[start..end].iter().any(|record| record.removed);
        let needs_reconstruction = group
            .iter()
            .any(|record| record_needs_reconstruction(record));

        // Buildifier keeps consecutive loads as a tight block. Detect any blank
        // line within the group's span so we still rewrite (and tighten) an
        // otherwise already-sorted group instead of leaving the blank lines.
        let has_internal_blank_lines = {
            let block_start = load_stmts[start].stmt_range.start();
            let block_end = load_stmt_end_of_last_line(&load_stmts[end - 1], module.source());
            module
                .source_text(TextRange::new(block_start, block_end))
                .lines()
                .any(|line| line.trim().is_empty())
        };

        if is_sorted && !has_removed_loads && !needs_reconstruction && !has_internal_blank_lines {
            continue; // Already sorted and tight
        }

        // Get the range for each load in the group including leading comments.
        // For the first load in the group, we need to find where leading comments start.
        // For subsequent loads, leading comments start from the end of the previous load's line.

        // Find where the first load's leading comments start by looking backwards
        // from the first load's line start to find where comments begin
        let first_load_line_start = {
            let stmt_start = load_stmts[start].stmt_range.start().to_usize();
            module.source()[..stmt_start]
                .rfind('\n')
                .map(|pos| pos + 1)
                .unwrap_or(0)
        };

        // Scan backwards from first_load_line_start to find consecutive comment/blank lines
        let group_start = TextSize::from(find_leading_comment_start(
            module.source(),
            first_load_line_start,
        ) as u32);

        // Track each kept load's index within the *full* records[start..end]
        // slice (i.e., its position before filtering removed records). We use
        // this when computing `prev_end` so that, when a removed (merged)
        // load sits between two kept loads, the kept load's range starts
        // *after* the removed load's source text — not after the previous
        // *kept* load. Otherwise the removed load's bytes get re-emitted
        // verbatim for kept loads that don't need reconstruction.
        let group_original_indices: Vec<usize> = records[start..end]
            .iter()
            .enumerate()
            .filter(|(_, record)| !record.removed)
            .map(|(offset, _)| start + offset)
            .collect();

        // Build ranges for each load including leading comments
        let load_ranges: Vec<TextRange> = group
            .iter()
            .enumerate()
            .map(|(i, record)| {
                let original_idx = group_original_indices[i];
                let prev_end = if original_idx == start {
                    group_start
                } else {
                    // Use the end of the previous *original* load's last line
                    // (not the previous kept load's). This way, any removed
                    // loads sitting between this kept load and the previous
                    // kept load are excluded from this load's source range,
                    // so their bytes don't bleed into the output.
                    load_stmt_end_of_last_line(records[original_idx - 1].load, module.source())
                };
                load_range_with_leading_comments(record.load, prev_end, module.source())
            })
            .collect();

        let group_end = load_stmt_end_of_last_line(&load_stmts[end - 1], module.source());

        // Extract source text for each load (including leading comments and trailing comments)
        // and build sorted replacement
        let sorted_loads: Vec<String> = sorted_indices
            .iter()
            .map(|&idx| {
                let record = group[idx];
                let range = load_ranges[idx];

                // Check if this load needs symbol sorting
                if record_needs_reconstruction(record) {
                    // Reconstruct with sorted symbols as single-line;
                    // Ruff will re-wrap if needed.
                    // Preserve leading comments but reconstruct the load itself.
                    let load_line_range = first_line_range_for_load(record.load, module);
                    let leading_comments =
                        if range.start() < load_line_range.start() {
                            strip_leading_blank_lines(module.source_text(TextRange::new(
                                range.start(),
                                load_line_range.start(),
                            )))
                        } else {
                            ""
                        };
                    format!(
                        "{}{}{}",
                        leading_comments,
                        render_record(record, module),
                        trailing_text_for_load(record.load, module)
                    )
                } else {
                    // No symbol sorting needed - use original source text to
                    // preserve comments, but drop any leading blank lines so the
                    // load block stays tight.
                    strip_leading_blank_lines(module.source_text(range))
                        .trim_end()
                        .to_owned()
                }
            })
            .collect();

        let replacement = sorted_loads.join("\n") + "\n";

        // Remove any symbol-sorting edits for loads in this group since we'll handle them together
        edits.retain(|e| e.range.start() < group_start || e.range.start() >= group_end);

        edits.push(Edit::new(
            TextRange::new(group_start, group_end),
            replacement,
        ));
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
    fn test_sort_load_paths() {
        let source = indoc! {r#"
            load("//z:z.bzl", "z_rule")
            load("//a:a.bzl", "a_rule")
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a_rule")
            load("//z:z.bzl", "z_rule")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_sort_symbols_within_load() {
        let source = "load(\":defs.bzl\", \"c_func\", \"a_func\", \"b_func\")\n";
        assert_eq!(
            run(source),
            "load(\":defs.bzl\", \"a_func\", \"b_func\", \"c_func\")\n"
        );
    }

    #[test]
    fn test_sort_multiline_load_preserves_separator_when_last_symbol_moves() {
        let source = indoc! {r#"
            load(
                "//pkg:defs.bzl",
                "z_func",
                "a_func"
            )
        "#};
        let expected = indoc! {r#"
            load(
                "//pkg:defs.bzl",
                "a_func",
                "z_func",
            )
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_keywords_after_positional() {
        let source =
            "load(\":defs.bzl\", \"b_func\", \"a_func\", a_renamed=\"a\", z_renamed=\"z\")\n";
        assert_eq!(
            run(source),
            "load(\":defs.bzl\", \"a_func\", \"b_func\", a_renamed=\"a\", z_renamed=\"z\")\n"
        );
    }

    #[test]
    fn test_already_sorted() {
        let source = indoc! {r#"
            load("//a:a.bzl", "a_rule")
            load("//b:b.bzl", "b_rule")
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_preserves_non_load_statements() {
        let source = indoc! {r#"
            load("//z:z.bzl", "z_rule")
            load("//a:a.bzl", "a_rule")
            x = 42
            load("//c:c.bzl", "c_rule")
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a_rule")
            load("//z:z.bzl", "z_rule")
            x = 42
            load("//c:c.bzl", "c_rule")
        "#};
        // First group sorted, second group (single load) unchanged
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_multiple_load_groups() {
        let source = indoc! {r#"
            load("//b:b.bzl", "b")
            load("//a:a.bzl", "a")
            x = 1
            load("//d:d.bzl", "d")
            load("//c:c.bzl", "c")
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a")
            load("//b:b.bzl", "b")
            x = 1
            load("//c:c.bzl", "c")
            load("//d:d.bzl", "d")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_buildifier_sort_order() {
        // Buildifier sorts: external (@) < cell-path (//) < relative (:)
        let source = indoc! {r#"
            load(":relative.bzl", "rel")
            load("@external//ext:ext.bzl", "ext")
            load("//local:local.bzl", "local")
        "#};
        let expected = indoc! {r#"
            load("@external//ext:ext.bzl", "ext")
            load("//local:local.bzl", "local")
            load(":relative.bzl", "rel")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_relative_loads_sort_last() {
        let source = indoc! {r#"
            load(":SOCKETS.bzl", "SOCKET_A")
            load("@fbsource//tools:defs.bzl", "tool")
            load("//lib:lib.bzl", "lib_func")
        "#};
        let expected = indoc! {r#"
            load("@fbsource//tools:defs.bzl", "tool")
            load("//lib:lib.bzl", "lib_func")
            load(":SOCKETS.bzl", "SOCKET_A")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_single_load_unchanged() {
        let source = "load(\":defs.bzl\", \"rule\")\n";
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_no_loads() {
        let source = "x = 42\n";
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_empty_module() {
        let source = "";
        assert_eq!(run(source), "");
    }

    #[test]
    fn test_complex_sorting() {
        let source = indoc! {r#"
            load("//z:z.bzl", "z_c", "z_a", z_renamed="z_orig")
            load("//a:a.bzl", "a_b", "a_a", a_alias="a_orig")
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a_a", "a_b", a_alias="a_orig")
            load("//z:z.bzl", "z_a", "z_c", z_renamed="z_orig")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_duplicate_symbols_are_removed() {
        let source = "load(\":defs.bzl\", \"b_func\", \"a_func\", \"a_func\")\n";
        assert_eq!(run(source), "load(\":defs.bzl\", \"a_func\", \"b_func\")\n");
    }

    #[test]
    fn test_same_origin_loads_are_compressed() {
        let source = indoc! {r#"
            load("//same:defs.bzl", "z_func")
            load("//other:defs.bzl", "other_func")
            load("//same:defs.bzl", "a_func")
        "#};
        let expected = indoc! {r#"
            load("//other:defs.bzl", "other_func")
            load("//same:defs.bzl", "a_func", "z_func")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_disable_same_origin_load_comment_prevents_compression() {
        let source = indoc! {r#"
            load("//same:defs.bzl", "z_func")
            load("//same:defs.bzl", "a_func")  # buildifier: disable=same-origin-load
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_same_origin_loads_in_separate_groups_are_not_merged() {
        // Loads from the same module in different groups (separated by a
        // non-load statement) must not be merged. Cross-group merging would
        // mark the second load as removed but skip the now-empty group's
        // edit, leaving orphaned load text in the output.
        let source = indoc! {r#"
            load("//same:defs.bzl", "z_func")

            x = 1

            load("//same:defs.bzl", "a_func")
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_disable_out_of_order_load_comment_preserves_load_order() {
        let source = indoc! {r#"
            # buildifier: disable=out-of-order-load
            load("//z:defs.bzl", "z_func", "a_func")
            load("//a:defs.bzl", "a_func")
        "#};
        let expected = indoc! {r#"
            # buildifier: disable=out-of-order-load
            load("//z:defs.bzl", "a_func", "z_func")
            load("//a:defs.bzl", "a_func")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_load_paths_compare_case_insensitively_by_path_chunks() {
        let source = indoc! {r#"
            load("//pkg/b:defs.bzl", "b")
            load("//pkg/A:defs.bzl", "a")
        "#};
        let expected = indoc! {r#"
            load("//pkg/A:defs.bzl", "a")
            load("//pkg/b:defs.bzl", "b")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_preserves_trailing_comment_when_sorting_load_group_and_symbols() {
        let source = indoc! {r#"
            load("//z:z.bzl", "z_b", "z_a")  # @unused
            load("//a:a.bzl", "a_b", "a_a")  # a comment
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a_a", "a_b")  # a comment
            load("//z:z.bzl", "z_a", "z_b")  # @unused
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_colon_sorts_before_slash() {
        let source = indoc! {r#"
            load("@repo//foo/bar:DEFS.bzl", "foo")
            load("@repo//tools/apple/plugins:plugin_defs.bzl", "plugin")
            load("@repo//tools/apple:lib.bzl", "lib")
            load("@repo//xplat/foundation:DEFS.bzl", "foundation")
        "#};
        let expected = indoc! {r#"
            load("@repo//foo/bar:DEFS.bzl", "foo")
            load("@repo//tools/apple:lib.bzl", "lib")
            load("@repo//tools/apple/plugins:plugin_defs.bzl", "plugin")
            load("@repo//xplat/foundation:DEFS.bzl", "foundation")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_colon_before_slash_simple() {
        let source = indoc! {r#"
            load("//a/c:d.bzl", "d")
            load("//a:b.bzl", "b")
        "#};
        let expected = indoc! {r#"
            load("//a:b.bzl", "b")
            load("//a/c:d.bzl", "d")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_colon_before_slash_idempotent() {
        let source = indoc! {r#"
            load("@repo//foo/bar:DEFS.bzl", "foo")
            load("@repo//tools/apple:lib.bzl", "lib")
            load("@repo//tools/apple/plugins:plugin_defs.bzl", "plugin")
            load("@repo//xplat/foundation:DEFS.bzl", "foundation")
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_inline_comment_preserved_when_already_sorted() {
        // When loads are already sorted, inline comments should be preserved
        let source = indoc! {r#"
            load("//a:a.bzl", "a")  # comment a
            load("//b:b.bzl", "b")  # comment b
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_leading_comment_preserved_when_already_sorted() {
        // When loads are already sorted, leading comments should be preserved
        let source = indoc! {r#"
            # Comment for a
            load("//a:a.bzl", "a")
            # Comment for b
            load("//b:b.bzl", "b")
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_single_load_with_inline_comment_preserved() {
        // Single load with inline comment should be unchanged
        let source = "load(\":defs.bzl\", \"rule\")  # important\n";
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_single_load_with_leading_comment_preserved() {
        // Single load with leading comment should be unchanged
        let source = indoc! {r#"
            # This is important
            load(":defs.bzl", "rule")
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_multiline_load_with_symbol_comments_sorted() {
        // Multiline loads with unsorted symbols are sorted by moving whole
        // symbol lines so inline comments stay attached.
        let source = indoc! {r#"
            load(
                ":defs.bzl",
                "c_func",  # comment c
                "a_func",  # comment a
                "b_func",  # comment b
            )
        "#};
        let expected = indoc! {r#"
            load(
                ":defs.bzl",
                "a_func",  # comment a
                "b_func",  # comment b
                "c_func",  # comment c
            )
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_multiline_load_already_sorted_preserves_comments() {
        // When multiline load symbols are already sorted, no change needed, comments preserved
        let source = indoc! {r#"
            load(
                ":defs.bzl",
                "a_func",  # comment a
                "b_func",  # comment b
            )
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_multiline_load_in_group_does_not_corrupt_next_load() {
        // Regression: when a multiline load is followed by single-line loads in the same
        // group, the next load's range was incorrectly computed from the multiline load's
        // first line (start) rather than its last line (end), causing the multiline load's
        // body to be included in the next load's "range" and corrupting the output.
        let source = indoc! {r#"
            load("//a:a.bzl", "a_func")
            load(
                "//b:b.bzl",
                "b_func",
                "b_other",
            )
            load("//c:c.bzl", "c_func")
        "#};
        // Already sorted - should be unchanged
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_multiline_load_in_group_reorder() {
        // Reordering a group that contains a multiline load
        let source = indoc! {r#"
            load("//z:z.bzl", "z_func")
            load(
                "//m:m.bzl",
                "m_func",
                "m_other",
            )
            load("//a:a.bzl", "a_func")
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a_func")
            load(
                "//m:m.bzl",
                "m_func",
                "m_other",
            )
            load("//z:z.bzl", "z_func")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_load_group_reorder_preserves_inline_comments() {
        // When loads are reordered, inline comments move with their loads
        let source = indoc! {r#"
            load("//z:z.bzl", "z")  # z comment
            load("//a:a.bzl", "a")  # a comment
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a")  # a comment
            load("//z:z.bzl", "z")  # z comment
        "#};
        // Comments are preserved and move with their respective loads
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_load_group_reorder_preserves_leading_comments() {
        // When loads are reordered, leading comments move with their loads
        let source = indoc! {r#"
            # Comment for z
            load("//z:z.bzl", "z")
            # Comment for a
            load("//a:a.bzl", "a")
        "#};
        let expected = indoc! {r#"
            # Comment for a
            load("//a:a.bzl", "a")
            # Comment for z
            load("//z:z.bzl", "z")
        "#};
        // Leading comments move with their respective loads
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_merged_load_text_does_not_bleed_into_next_load() {
        // Regression for the orphaned-source-text bug: when a same-origin
        // load is merged into an earlier load and a later load follows it,
        // the later load's range used to start from the previous *kept* load
        // instead of the previous *original* load. As a result, the merged
        // (removed) load's bytes were re-emitted between the two kept loads.
        //
        // The "sandwich" pattern: a merged load sits between two kept loads.
        let source = indoc! {r#"
            load("//a:a.bzl", "x")
            load("//a:a.bzl", "z")
            load("//b:b.bzl", "y")
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "x", "z")
            load("//b:b.bzl", "y")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_blank_line_between_sorted_loads_is_removed() {
        // Buildifier keeps consecutive loads as a tight block with no blank
        // lines between them, so a blank line between two already-sorted loads
        // is removed even though the loads need no reordering.
        let source = indoc! {r#"
            load("//a:a.bzl", "a")

            load("//b:b.bzl", "b")
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a")
            load("//b:b.bzl", "b")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_blank_lines_between_unsorted_loads_are_removed() {
        // Reordering already removed blank lines; this guards the multi-load
        // case where the tightening must hold across the whole block.
        let source = indoc! {r#"
            load("//c:c.bzl", "c")

            load("//a:a.bzl", "a")

            load("//b:b.bzl", "b")
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a")
            load("//b:b.bzl", "b")
            load("//c:c.bzl", "c")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_blank_line_between_sorted_loads_preserves_leading_comments() {
        // Tightening the block must not drop a load's leading comment.
        let source = indoc! {r#"
            load("//a:a.bzl", "a")

            # comment for b
            load("//b:b.bzl", "b")
        "#};
        let expected = indoc! {r#"
            load("//a:a.bzl", "a")
            # comment for b
            load("//b:b.bzl", "b")
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_dedup_keeps_first_occurrence_of_symbol() {
        // Regression: when the same bound name appears as both a positional
        // and a keyword alias, the first occurrence wins (matches Buildifier).
        // Previously the dedup popped the previous and pushed the later, so
        // the alias `foo="bar"` silently replaced the positional `"foo"`.
        let source = "load(\":a.bzl\", \"foo\", foo=\"bar\")\n";
        assert_eq!(run(source), "load(\":a.bzl\", \"foo\")\n");
    }

    #[test]
    fn test_file_header_preserved() {
        let source = indoc! {r#"
            # File header

            # License

            load("//z:z.bzl", "z")
            load("@fbsource//a:a.bzl", "a")

            x = a() + z()
        "#};
        let expected = indoc! {r#"
            # File header

            # License

            load("@fbsource//a:a.bzl", "a")
            load("//z:z.bzl", "z")

            x = a() + z()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_file_header_preserved_immediate_comment_moved() {
        let source = indoc! {r#"
            # File header

            # License

            # Comment for z
            load("//z:z.bzl", "z")
            # Comment for a
            load("@fbsource//a:a.bzl", "a")

            x = a() + z()
        "#};
        let expected = indoc! {r#"
            # File header

            # License

            # Comment for a
            load("@fbsource//a:a.bzl", "a")
            # Comment for z
            load("//z:z.bzl", "z")

            x = a() + z()
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_file_header_preserved_stray_comments_moved() {
        let source = indoc! {r#"
            # File header

            # License

            # Comment for a
            load("@fbsource//a:a.bzl", "a")
            # Stray comment for z


            # Comment for z
            load("//z:z.bzl", "z")


            # Stray comment for b


            # Comment for b

            load("@fbsource//b:b.bzl", "b")

            x = a() + b() + z()
        "#};
        let expected = indoc! {r#"
            # File header

            # License

            # Comment for a
            load("@fbsource//a:a.bzl", "a")
            # Stray comment for b


            # Comment for b

            load("@fbsource//b:b.bzl", "b")
            # Stray comment for z


            # Comment for z
            load("//z:z.bzl", "z")

            x = a() + b() + z()
        "#};
        assert_eq!(run(source), expected);
    }
}
