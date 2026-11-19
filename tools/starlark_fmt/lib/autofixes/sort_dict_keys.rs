/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Sorts dictionary literal keys using buildifier's `unsorted-dict-items` ordering.
//!
//! By default, all dictionary literals have their string keys sorted. Non-string
//! keys (variables, expressions) are left in place, and string-keyed entries are
//! sorted across the remaining string-key slots.
//!
//! Starred elements (e.g., `**kwargs`) remain in their original positions since
//! their keys are not statically known.
//!
//! To disable sorting for a specific dictionary, add `@unsorted-dict-items`
//! in a comment before or at the start of the dictionary literal:
//!
//! ```starlark
//! # @unsorted-dict-items
//! {"z": 1, "a": 2}  # Keys preserved as-is
//! ```
//!
//! Or on the same line as the opening brace:
//!
//! ```starlark
//! {  # @unsorted-dict-items
//!     "z": 1,
//!     "a": 2,
//! }
//! ```
//!
//! ```starlark
//! # Before:
//! {"z": 1, "_private": 4, "m": 2, "a": 3}
//!
//! # After:
//! {"a": 3, "m": 2, "z": 1, "_private": 4}
//! ```

use std::collections::HashSet;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprDict;
use ruff_python_ast::Stmt;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::visitor::walk_expr;
use ruff_python_ast::visitor::walk_stmt;
use ruff_text_size::Ranged;
use ruff_text_size::TextRange;
use ruff_text_size::TextSize;

use super::parsed_module::Edit;
use super::parsed_module::ParsedModule;
use super::utils::block_ranges_valid;
use super::utils::block_with_trailing_comma;
use super::utils::compute_block_ranges;
use super::utils::find_line_end;
use super::utils::line_start;
use super::utils::offset_past_newline;

const UNSORTED_DICT_ITEMS_ANNOTATION: &str = "@unsorted-dict-items";

const CONDITIONS_DEFAULT_KEY: &str = "//conditions:default";

fn is_default_condition_key(key: &str) -> bool {
    key == CONDITIONS_DEFAULT_KEY
}

/// Sort key for dict entries.
///
/// Order:
/// 1. Leading-underscore keys last.
/// 2. `//conditions:default` last among regular keys.
/// 3. Raw lexicographic key order.
fn dict_key_sort_key(key: &str) -> (bool, bool, &str) {
    (key.starts_with('_'), is_default_condition_key(key), key)
}

/// Represents a dict entry for sorting purposes.
#[derive(Debug, Clone, Copy)]
enum DictEntry<'a> {
    /// Starred element: `**expr`
    Starred { source_text: &'a str },
    /// Entry with a string literal key (sortable)
    StringKey { key: &'a str, source_text: &'a str },
    /// Entry with a non-string key (variable, integer, etc.) - fixed slot
    OtherKey { source_text: &'a str },
}

impl<'a> DictEntry<'a> {
    /// Returns the source text for this entry.
    fn source_text(&self) -> &'a str {
        match self {
            DictEntry::Starred { source_text }
            | DictEntry::StringKey { source_text, .. }
            | DictEntry::OtherKey { source_text } => source_text,
        }
    }

    /// Returns the string key if this is a sortable string-keyed entry.
    fn string_key(&self) -> Option<&'a str> {
        match self {
            DictEntry::StringKey { key, .. } => Some(key),
            _ => None,
        }
    }
}

fn comments_in_range_contain_unsorted_annotation(range: TextRange, module: &ParsedModule) -> bool {
    module
        .comments_in_range(range)
        .any(|comment| comment.contains(UNSORTED_DICT_ITEMS_ANNOTATION))
}

fn previous_line_range(source: &str, line_start: usize) -> Option<TextRange> {
    if line_start == 0 {
        return None;
    }

    let prev_line_end = line_start - 1;
    let prev_line_start = source[..prev_line_end]
        .rfind('\n')
        .map(|p| p + 1)
        .unwrap_or(0);
    Some(TextRange::new(
        TextSize::from(prev_line_start as u32),
        TextSize::from(prev_line_end as u32),
    ))
}

fn previous_line_has_standalone_unsorted_annotation(
    offset: TextSize,
    module: &ParsedModule,
) -> bool {
    let Some(prev_line_range) = previous_line_range(
        module.source(),
        line_start(module.source(), offset.to_usize()),
    ) else {
        return false;
    };

    let prev_line = module.source_text(prev_line_range).trim();
    prev_line.starts_with('#') && prev_line.contains(UNSORTED_DICT_ITEMS_ANNOTATION)
}

fn line_comments_contain_unsorted_annotation(start: TextSize, module: &ParsedModule) -> bool {
    comments_in_range_contain_unsorted_annotation(
        TextRange::new(start, module.line_end(start)),
        module,
    )
}

/// Check if a dict has the `@unsorted-dict-items` annotation.
/// Checks comments on the same line as the dict start, the line before the `{`,
/// and also the line before the statement containing the dict (for patterns like
/// `# @unsorted-dict-items\nPRIORITY_MAP = {`).
fn has_unsorted_annotation(dict_start: TextSize, module: &ParsedModule) -> bool {
    previous_line_has_standalone_unsorted_annotation(dict_start, module)
        || line_comments_contain_unsorted_annotation(dict_start, module)
}

/// Check if a dict item has the `@unsorted-dict-items` annotation.
///
/// Item-level annotations apply to all dicts inside that item's value
/// expression. Unlike `has_unsorted_annotation`, this only treats the previous
/// line as an annotation when it is a standalone comment line, so a trailing
/// annotation on one item does not suppress the next sibling item.
fn has_unsorted_item_annotation(
    item_start: TextSize,
    item_value_end: TextSize,
    module: &ParsedModule,
) -> bool {
    previous_line_has_standalone_unsorted_annotation(item_start, module)
        || line_comments_contain_unsorted_annotation(item_start, module)
        || line_comments_contain_unsorted_annotation(item_value_end, module)
}

/// Collect dict entries from a Dict expression.
fn collect_dict_entries<'a>(dict: &'a ExprDict, source: &'a str) -> Vec<DictEntry<'a>> {
    dict.items
        .iter()
        .map(|item| {
            if let Some(key) = &item.key {
                // Regular key: value entry
                let range = TextRange::new(key.range().start(), item.value.range().end());
                let source_text = source[range].trim();

                match key {
                    Expr::StringLiteral(s) => DictEntry::StringKey {
                        // `s.value.to_str()` returns the already-parsed string
                        // content (no surrounding quotes). Do NOT re-run
                        // `unquote()` here — that would strip quote *characters*
                        // from keys like `'"'`, collapsing the sort key to `""`.
                        key: s.value.to_str(),
                        source_text,
                    },
                    _ => DictEntry::OtherKey { source_text },
                }
            } else {
                // Starred element: need to include "**" prefix
                let value_start = item.value.range().start().to_usize();
                let star_start =
                    if value_start >= 2 && source[value_start - 2..value_start] == *"**" {
                        value_start - 2
                    } else {
                        value_start
                    };
                let range =
                    TextRange::new(TextSize::from(star_start as u32), item.value.range().end());
                DictEntry::Starred {
                    source_text: source[range].trim(),
                }
            }
        })
        .collect()
}

/// Sort dict entries in place, preserving starred elements and non-string key slots.
/// Returns true if any sorting was done.
fn sort_entries(entries: &mut [DictEntry<'_>]) -> bool {
    if entries.len() < 2 {
        return false;
    }

    let string_key_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.string_key().map(|_| index))
        .collect();
    if string_key_indices.len() < 2 {
        return false;
    }

    let needs_sort = string_key_indices.windows(2).any(|pair| {
        let key_a = entries[pair[0]]
            .string_key()
            .expect("sortable dict entries have string keys");
        let key_b = entries[pair[1]]
            .string_key()
            .expect("sortable dict entries have string keys");
        dict_key_sort_key(key_a) > dict_key_sort_key(key_b)
    });

    if !needs_sort {
        return false;
    }

    let mut sorted_entries: Vec<DictEntry<'_>> = string_key_indices
        .iter()
        .map(|&index| entries[index])
        .collect();
    sorted_entries.sort_by_key(|entry| {
        entry
            .string_key()
            .map(dict_key_sort_key)
            .expect("sortable dict entries have string keys")
    });

    for (slot_index, sorted_entry) in string_key_indices.into_iter().zip(sorted_entries) {
        entries[slot_index] = sorted_entry;
    }

    true
}

/// Process a single dict and return an edit if sorting is needed.
fn process_dict(dict: &ExprDict, module: &ParsedModule) -> Option<Edit> {
    let source = module.source();
    let mut entries = collect_dict_entries(dict, source);

    if entries.len() < 2 {
        return None;
    }

    // Try to sort
    if !sort_entries(&mut entries) {
        return None;
    }

    // Reconstruct the dict
    let dict_source = module.source_text(dict.range());
    let is_multiline = dict_source.contains('\n');

    if is_multiline {
        process_dict_multiline(dict, module).or_else(|| {
            if module.comments_in_range(dict.range()).next().is_none() {
                process_dict_singleline(dict, module, &entries)
            } else {
                None
            }
        })
    } else {
        process_dict_singleline(dict, module, &entries)
    }
}

/// Process a single-line dict: reconstruct from sorted entry source texts.
fn process_dict_singleline(
    dict: &ExprDict,
    module: &ParsedModule,
    entries: &[DictEntry<'_>],
) -> Option<Edit> {
    let dict_source = module.source_text(dict.range());
    let open_brace = dict_source.find('{')?;

    let before_first_entry = if let Some(first_item) = dict.items.first() {
        let first_range = if let Some(key) = &first_item.key {
            key.range().start()
        } else {
            first_item.value.range().start()
        };
        let offset = first_range.to_usize() - dict.range().start().to_usize();
        &dict_source[..offset]
    } else {
        &dict_source[..=open_brace]
    };

    let close_brace = dict_source.rfind('}')?;
    let after_entries = &dict_source[close_brace..];

    let entry_texts: Vec<&str> = entries.iter().map(|e| e.source_text()).collect();
    let result = format!(
        "{}{}{}",
        before_first_entry,
        entry_texts.join(", "),
        after_entries
    );

    Some(Edit::new(dict.range(), result))
}

/// Process a multiline dict using line-based block ranges to preserve comments.
///
/// Each entry's "block" spans from the line after the previous entry to the end
/// of this entry's line, capturing leading comments and trailing inline comments.
fn process_dict_multiline(dict: &ExprDict, module: &ParsedModule) -> Option<Edit> {
    let source = module.source();
    let items = &dict.items;

    if items.len() < 2 {
        return None;
    }

    // Compute block ranges for each item
    let brace_line_end = find_line_end(source, dict.range().start());
    let first_block_start = offset_past_newline(source, brace_line_end);
    let element_ends: Vec<TextSize> = items.iter().map(|item| item.value.range().end()).collect();
    let block_ranges = compute_block_ranges(source, first_block_start, &element_ends);

    if !block_ranges_valid(&block_ranges, source) {
        return None;
    }

    // Get original keys for matching
    let original_keys: Vec<Option<&str>> = items
        .iter()
        .map(|item| {
            item.key.as_ref().and_then(|k| {
                if let Expr::StringLiteral(s) = k {
                    Some(s.value.to_str())
                } else {
                    None
                }
            })
        })
        .collect();

    // Sort string-keyed blocks across all string-key slots while preserving
    // non-string and starred slots.
    let entries_start = block_ranges[0].start();
    let entries_end = block_ranges[block_ranges.len() - 1].end();

    let string_key_indices: Vec<usize> = original_keys
        .iter()
        .enumerate()
        .filter_map(|(index, key)| key.map(|_| index))
        .collect();
    if string_key_indices.len() < 2 {
        return None;
    }

    let mut sorted_indices = string_key_indices.clone();
    sorted_indices.sort_by_key(|&index| {
        dict_key_sort_key(original_keys[index].expect("sortable dict entries have string keys"))
    });

    if sorted_indices == string_key_indices {
        return None;
    }

    let mut sorted_index_iter = sorted_indices.into_iter();
    let last_index = original_keys.len() - 1;
    let mut replacement = String::new();
    for (index, original_key) in original_keys.iter().enumerate() {
        let source_index = if original_key.is_some() {
            sorted_index_iter
                .next()
                .expect("sorted indices match string-key slots")
        } else {
            index
        };
        let block_range = block_ranges[source_index];
        let block = &source[block_range];
        // A block moved to a non-last position must end with a comma. The
        // original last entry can legally omit its trailing comma; once it is
        // relocated ahead of another entry, the missing comma would splice two
        // entries together (e.g. `"a": None` directly followed by `"z": 1`),
        // producing source that fails to re-parse. A `None` return means the
        // value end fell outside its block — unreachable for AST-derived ranges,
        // but if it ever happens, abandon the whole reorder rather than emit a
        // block that might splice.
        if index < last_index {
            match block_with_trailing_comma(
                source,
                block_range,
                items[source_index].value.range().end(),
            ) {
                Some(block_text) => replacement.push_str(&block_text),
                None => return None,
            }
        } else {
            replacement.push_str(block);
        }
    }

    let edit_range = TextRange::new(entries_start, entries_end);

    // Check if anything actually changed
    if replacement == source[edit_range] {
        return None;
    }

    Some(Edit::new(edit_range, replacement))
}

/// Visitor that collects every dict expression and notes which ones inherit
/// the `@unsorted-dict-items` annotation from an enclosing scope. The visitor's
/// own recursion stack *is* the scope stack, so a single counter (incremented
/// when entering an annotated dict, decremented when leaving it) tells us at
/// each dict whether suppression is active.
struct DictCollector<'a> {
    dicts: Vec<&'a ExprDict>,
    unsorted_dict_starts: HashSet<TextSize>,
    suppression_depth: u32,
    module: &'a ParsedModule<'a>,
}

impl<'a> Visitor<'a> for DictCollector<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        let Expr::Dict(dict) = expr else {
            walk_expr(self, expr);
            return;
        };

        let start = dict.range().start();
        let has_annotation = has_unsorted_annotation(start, self.module);

        self.dicts.push(dict);
        if has_annotation || self.suppression_depth > 0 {
            self.unsorted_dict_starts.insert(start);
        }

        if has_annotation {
            self.suppression_depth += 1;
        }

        for item in &dict.items {
            if let Some(key) = &item.key {
                self.visit_expr(key);
            }

            let has_item_annotation = item.key.as_ref().map_or_else(
                || {
                    has_unsorted_item_annotation(
                        item.value.range().start(),
                        item.value.range().end(),
                        self.module,
                    )
                },
                |key| {
                    has_unsorted_item_annotation(
                        key.range().start(),
                        item.value.range().end(),
                        self.module,
                    )
                },
            );

            if has_item_annotation {
                self.suppression_depth += 1;
                self.visit_expr(&item.value);
                self.suppression_depth -= 1;
            } else {
                self.visit_expr(&item.value);
            }
        }

        if has_annotation {
            self.suppression_depth -= 1;
        }
    }
}

/// Find all dicts in DFS pre-order and note which ones are under an
/// `@unsorted-dict-items` suppression (their own or an enclosing dict's).
fn find_all_dicts<'a>(
    stmts: &'a [Stmt],
    module: &'a ParsedModule<'a>,
) -> (Vec<&'a ExprDict>, HashSet<TextSize>) {
    let mut collector = DictCollector {
        dicts: Vec::new(),
        unsorted_dict_starts: HashSet::new(),
        suppression_depth: 0,
        module,
    };
    for stmt in stmts {
        walk_stmt(&mut collector, stmt);
    }
    (collector.dicts, collector.unsorted_dict_starts)
}

/// Collect edits that sort dictionary keys.
///
/// Nested dicts are fully supported via multiple passes. In each pass, we process
/// inner dicts first and skip outer dicts that contain edited inner dicts (to avoid
/// range invalidation). The caller (`apply_autofixes`) runs this transform multiple
/// times until no more changes are made, ensuring all nested dicts get sorted.
///
/// `@unsorted-dict-items` propagates from an enclosing dict to all nested dicts
/// inside it — without this, an outer suppression on a structure like
/// `{... # @unsorted-dict-items \n "k": {...}}` would still let the inner dict
/// get reordered, breaking downstream code that depends on insertion order.
pub fn collect_edits(module: &ParsedModule) -> Vec<Edit> {
    let stmts = module.stmts();
    if stmts.is_empty() {
        return Vec::new();
    }

    // Single AST walk produces both the dict list (DFS pre-order, so parents
    // precede their descendants) and the set of dicts under `@unsorted-dict-items`
    // suppression. No sort or post-pass needed.
    let (all_dicts, unsorted_dict_starts) = find_all_dicts(stmts, module);
    if all_dicts.is_empty() {
        return Vec::new();
    }

    // Process inner dicts first (reverse DFS pre-order) to avoid range
    // invalidation when an outer dict's text contains a just-edited inner one.
    let mut edits = Vec::new();
    let mut processed_ranges: Vec<(TextSize, TextSize)> = Vec::new();
    for dict in all_dicts.into_iter().rev() {
        if unsorted_dict_starts.contains(&dict.range().start()) {
            continue;
        }

        let dict_range = dict.range();
        let contains_edited_dict = processed_ranges
            .iter()
            .any(|&(start, end)| dict_range.start() < start && end < dict_range.end());

        if contains_edited_dict {
            continue;
        }

        if let Some(edit) = process_dict(dict, module) {
            processed_ranges.push((dict_range.start(), dict_range.end()));
            edits.push(edit);
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
        let mut result = source.to_owned();
        // Run multiple times to handle nested dicts
        for _ in 0..5 {
            let prev = result.clone();
            result = ParsedModule::parse(Cow::Owned(result))
                .and_then(|module| module.run_transform(collect_edits))
                .expect("failed to collect edits")
                .unparse();
            if result == prev {
                break;
            }
        }
        result
    }

    #[test]
    fn test_basic_sorting() {
        let source = r#"x = {"z": 1, "a": 2, "m": 3}"#;
        let result = run(source);
        assert!(result.contains(r#""a": 2"#));
        assert!(result.contains(r#""m": 3"#));
        assert!(result.contains(r#""z": 1"#));
        // Check order
        let a_pos = result.find(r#""a""#).unwrap();
        let m_pos = result.find(r#""m""#).unwrap();
        let z_pos = result.find(r#""z""#).unwrap();
        assert!(a_pos < m_pos && m_pos < z_pos);
    }

    #[test]
    fn test_leading_underscore_keys_sort_last_single_line() {
        let source = r#"x = {"source_listing": 1, "_processor": 2, "include_from_file": 3}"#;
        let expected = r#"x = {"include_from_file": 3, "source_listing": 1, "_processor": 2}"#;
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_leading_underscore_key_stays_after_uppercase_key() {
        let source = indoc! {r#"
            A = {
                "B": "fbcode/buck2/**",
                "_A": "fbcode/antlir/**",
            }
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_already_sorted() {
        let source = r#"x = {"a": 1, "b": 2, "c": 3}"#;
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_mixed_inline_multiline_dict_sorts_before_formatting() {
        let source = indoc! {r#"
            x = select({"DEFAULT": [], "ovr_config//os:windows": [
                "-DUNICODE",
                "-D_UNICODE",
            ]})
        "#};
        let expected = indoc! {r#"
            x = select({"DEFAULT": [], "ovr_config//os:windows": [
                "-DUNICODE",
                "-D_UNICODE",
            ]})
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_single_element() {
        let source = r#"x = {"only": 1}"#;
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_empty_dict() {
        let source = "x = {}";
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_unsorted_annotation_inline() {
        let source = "x = {  # @unsorted-dict-items\n    \"z\": 1, \"a\": 2}";
        // Should preserve original order
        let result = run(source);
        let z_pos = result.find(r#""z""#).unwrap();
        let a_pos = result.find(r#""a""#).unwrap();
        assert!(z_pos < a_pos, "z should come before a (unsorted)");
    }

    #[test]
    fn test_unsorted_annotation_preceding_line() {
        // @unsorted-dict-items on the line before the assignment
        let source = indoc! {r#"
            # @unsorted-dict-items
            PRIORITY_MAP = {
                "z_first": 1,
                "a_second": 2,
            }
        "#};
        let result = run(source);
        let z_pos = result.find("\"z_first\"").unwrap();
        let a_pos = result.find("\"a_second\"").unwrap();
        assert!(
            z_pos < a_pos,
            "z_first should come before a_second (unsorted annotation on preceding line)"
        );
    }

    #[test]
    fn test_starred_preserved() {
        let source = r#"x = {"c": 1, **kwargs, "b": 2, "a": 3}"#;
        let result = run(source);
        // **kwargs should be preserved
        assert!(result.contains("**kwargs"));
        // c stays first (only element before **kwargs)
        // a, b sorted after **kwargs
        assert!(result.contains(r#""c": 1"#));
    }

    #[test]
    fn test_non_string_keys_preserved() {
        let source = r#"x = {variable: 1, "z": 2, "a": 3}"#;
        let result = run(source);
        // variable key should stay first, string keys sorted
        assert!(result.contains("variable: 1"));
        let a_pos = result.find(r#""a""#).unwrap();
        let z_pos = result.find(r#""z""#).unwrap();
        assert!(a_pos < z_pos);
    }

    #[test]
    fn test_nested_dicts() {
        let source = r#"x = {"z": {"b": 2, "a": 1}, "m": {"d": 4, "c": 3}}"#;
        let result = run(source);
        // Both outer and inner dicts should be sorted
        // Outer: m before z
        let m_pos = result.find(r#""m""#).unwrap();
        let z_pos = result.find(r#""z""#).unwrap();
        assert!(m_pos < z_pos, "outer dict should be sorted: m < z");
    }

    #[test]
    fn test_no_dict() {
        let source = "x = 42\n";
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_integer_keys_not_sorted() {
        // Buildifier does NOT sort integer keys.
        let source = "x = {3: \"three\", 1: \"one\", 2: \"two\"}";
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_float_keys_not_sorted() {
        // Buildifier does NOT sort float keys.
        let source = "x = {3.14: \"pi\", 1.0: \"one\", 2.71: \"e\"}";
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_mixed_string_and_int_keys() {
        let source = r#"x = {"z": 1, 3: "three", "a": 2, 1: "one"}"#;
        let expected = r#"x = {"a": 2, 3: "three", "z": 1, 1: "one"}"#;
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_string_keys_sorted_across_int_fixed_slots() {
        let source = r#"x = {"z": 1, "b": 2, 3: "barrier", "y": 4, "a": 5}"#;
        let expected = r#"x = {"a": 5, "b": 2, 3: "barrier", "y": 4, "z": 1}"#;
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_multiline_inline_comments_preserved() {
        // Trailing inline comments should move with their entries
        let source = indoc! {r#"
            x = {
                "z_key": "z_value",  # z comment
                "a_key": "a_value",  # a comment
            }
        "#};
        let expected = indoc! {r#"
            x = {
                "a_key": "a_value",  # a comment
                "z_key": "z_value",  # z comment
            }
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_leading_underscore_keys_sort_last_multiline() {
        let source = indoc! {r#"
            attrs = {
                "source_listing": attrs.dep(),
                "_processor": attrs.exec_dep(),
                "include_from_file": attrs.source(),
            }
        "#};
        let expected = indoc! {r#"
            attrs = {
                "include_from_file": attrs.source(),
                "source_listing": attrs.dep(),
                "_processor": attrs.exec_dep(),
            }
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_multiline_leading_comments_as_group_separators() {
        // Leading comments on separate lines mean each entry is its own group.
        // Since we sort blocks including leading content, comments stay with their entry.
        let source = indoc! {r#"
            x = {
                # z section
                "z_key": "z_value",
                # a section
                "a_key": "a_value",
            }
        "#};
        let expected = indoc! {r#"
            x = {
                # a section
                "a_key": "a_value",
                # z section
                "z_key": "z_value",
            }
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_multiline_mixed_comments_preserved() {
        // Both leading and inline comments should be preserved
        let source = indoc! {r#"
            x = {
                # z section
                "z_key": "z_value",  # z inline
                # a section
                "a_key": "a_value",  # a inline
            }
        "#};
        let expected = indoc! {r#"
            x = {
                # a section
                "a_key": "a_value",  # a inline
                # z section
                "z_key": "z_value",  # z inline
            }
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_multiline_already_sorted_with_comments() {
        // Already sorted: no changes, comments preserved
        let source = indoc! {r#"
            x = {
                "a_key": "a_value",  # a comment
                "z_key": "z_value",  # z comment
            }
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_unsorted_annotation_propagates_to_nested_dicts() {
        // The downstream code in coleman2_soc_0P7_memory_map.bzl relies on
        // insertion order in inner dicts; the outer @unsorted-dict-items must
        // suppress sorting in nested dicts too.
        let source = indoc! {r#"
            MEMORY_MAP = {  # @unsorted-dict-items
                "region_a": {
                    "start": 1,
                    "end": 2,
                },
                "region_b": {
                    "start": 3,
                    "end": 4,
                },
            }
        "#};
        let result = run(source);
        assert_eq!(
            result, source,
            "outer @unsorted-dict-items should propagate to nested dicts"
        );
    }

    #[test]
    fn test_unsorted_annotation_propagates_to_deeply_nested_dicts() {
        let source = indoc! {r#"
            CONFIG = {  # @unsorted-dict-items
                "outer": {
                    "middle": {
                        "z": 1,
                        "a": 2,
                    },
                },
            }
        "#};
        let result = run(source);
        assert_eq!(
            result, source,
            "outer @unsorted-dict-items should propagate to grandchild dicts"
        );
    }

    #[test]
    fn test_unsorted_annotation_on_dict_item_propagates_to_value_expression() {
        let source = indoc! {r#"
            MOCK = {
                # @unsorted-dict-items
                "value": python.json_dumps([{
                    "project_configs": [
                        {
                            "name": "fake1",
                            "include_globs": [
                                "fbcode/fake1/**",
                            ],
                            "exclude_globs": [
                                "fbcode/fake1/exclude/**",
                            ],
                        },
                        {
                            "name": "fake2",
                            "include_globs": [
                                "fbcode/fake2",
                            ],
                            "exclude_globs": [
                            ],
                        },
                    ],
                }]),
            }
        "#};
        assert_eq!(
            run(source),
            source,
            "item-level @unsorted-dict-items should suppress all dicts in the value expression"
        );
    }

    #[test]
    fn test_trailing_item_annotation_does_not_suppress_next_sibling() {
        let source = indoc! {r#"
            MOCK = {
                "first": python.json_dumps([{"z": 1, "a": 2}]),  # @unsorted-dict-items
                "second": {"z": 1, "a": 2},
            }
        "#};
        let expected = indoc! {r#"
            MOCK = {
                "first": python.json_dumps([{"z": 1, "a": 2}]),  # @unsorted-dict-items
                "second": {"a": 2, "z": 1},
            }
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_quote_char_keys_sort_by_actual_content() {
        // Regression: keys whose CONTENT is a quote char (e.g. `'"'` or `"'"`)
        // must sort by the parsed character, not the empty string. A previous
        // bug ran `unquote()` on the already-unquoted parsed value and
        // collapsed both `'"'` (`"`) and `"'"` (`'`) to `""`.
        let source = indoc! {r##"
            _ESCAPE_MAP = {
                " ": "x20",
                "!": "x21",
                '"': "x22",
                "#": "x23",
                "&": "x26",
                "'": "x27",
                "(": "x28",
            }
        "##};
        // All these keys are single-char punctuation and sort by raw ASCII
        // among themselves.
        // The input is already in ascending ASCII order, so no reordering.
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_cell_path_key_sorts_before_at_external_key() {
        // Buildifier's dict lint orders `//...` before `@...`
        // lexicographically.
        let source = indoc! {r#"
            x = {
                "//condition": [],
                "@bazel_tools//": [],
            }
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_select_branches_use_buildifier_dict_order() {
        // Dict keys use raw lexicographic order after private-key-last and
        // `//conditions:default`-last handling. `DEFAULT` is an ordinary key.
        let source = indoc! {r#"
            deps = select({
                "//condition:linux": [],
                "//condition:windows": [],
                "@bazel_tools//src/conditions:darwin": [],
                "DEFAULT": [],
            })
        "#};
        let expected = indoc! {r#"
            deps = select({
                "//condition:linux": [],
                "//condition:windows": [],
                "@bazel_tools//src/conditions:darwin": [],
                "DEFAULT": [],
            })
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_external_only_select_with_default_branch_uses_buildifier_order() {
        // `//conditions:default` sorts after regular keys.
        let source = indoc! {r#"
            linkopts = select({
                "@repo//bazel:os_linux": ["-ldl", "-lrt", "-lpthread"],
                "@repo//bazel:os_osx": [],
                "@repo//bazel:os_windows": [],
                "//conditions:default": ["-ldl"],
            })
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_relative_select_branch_sorts_before_default_condition() {
        let source = indoc! {r#"
            copts = select({
                "//conditions:default": [],
                ":clang-cl": clang_cl_only_copts,
            })
        "#};
        let expected = indoc! {r#"
            copts = select({
                ":clang-cl": clang_cl_only_copts,
                "//conditions:default": [],
            })
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_cell_path_select_branches_keep_default_condition_last() {
        let source = indoc! {r#"
            copts = select({
                "//opencensus:llvm_compiler": ABSL_LLVM_FLAGS,
                "//opencensus:windows": ABSL_MSVC_FLAGS,
                "//conditions:default": ABSL_GCC_FLAGS,
            })
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_default_condition_uses_buildifier_order_outside_select() {
        let source = indoc! {r#"
            data = {
                ":clang-cl": clang_cl_only_copts,
                "//conditions:default": [],
            }
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_default_key_uses_raw_lexicographic_order_outside_select() {
        let source = indoc! {r#"
            data = {
                "DEFAULT": [],
                "ovr_config//os:windows": [],
            }
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_default_select_branch_uses_raw_lexicographic_order() {
        let source = indoc! {r#"
            copts = select({
                "DEFAULT": [],
                "ovr_config//os:windows": [],
            })
        "#};
        assert_eq!(run(source), source);
    }

    #[test]
    fn test_external_label_map_like_keys_use_buildifier_order() {
        // All keys share an external-label shape, so this confirms raw
        // lexicographic ordering for `//`, `/`, and `:` inside the key.
        let source = indoc! {r#"
            EXTERNAL_LABEL_MAP = {
                "@repo_delta/pkg:target_z": "workspace//vendor/repo_delta/pkg:target_z",
                "@repo_charlie//:lib": "workspace//vendor/repo_charlie:lib",
                "@repo_hotel": "workspace//vendor/repo_hotel:default",
                "@repo_bravo//:tool_plus": "workspace//vendor/repo_bravo:tool_plus",
                "@repo_alpha//tools/cpp/runfiles:runfiles": "workspace//stubs:runfiles",
                "@repo_delta//pkg:target_a": "workspace//vendor/repo_delta/pkg:target_a",
                "@repo_india": None,
                "@repo_charlie_benchmark//:benchmark": "workspace//benchmarks:benchmark",
                "@repo_bravo//:tool": "workspace//vendor/repo_bravo:tool",
                "@repo_hotel//:sip_hash": "workspace//vendor/repo_hotel:sip_hash",
                "@repo_echo//:eigen": "workspace//vendor/repo_echo:eigen",
                "@repo_tango//pkg/platform:protobuf": "workspace//vendor/repo_tango/pkg/platform:protobuf",
            }
        "#};
        let expected = indoc! {r#"
            EXTERNAL_LABEL_MAP = {
                "@repo_alpha//tools/cpp/runfiles:runfiles": "workspace//stubs:runfiles",
                "@repo_bravo//:tool": "workspace//vendor/repo_bravo:tool",
                "@repo_bravo//:tool_plus": "workspace//vendor/repo_bravo:tool_plus",
                "@repo_charlie//:lib": "workspace//vendor/repo_charlie:lib",
                "@repo_charlie_benchmark//:benchmark": "workspace//benchmarks:benchmark",
                "@repo_delta//pkg:target_a": "workspace//vendor/repo_delta/pkg:target_a",
                "@repo_delta/pkg:target_z": "workspace//vendor/repo_delta/pkg:target_z",
                "@repo_echo//:eigen": "workspace//vendor/repo_echo:eigen",
                "@repo_hotel": "workspace//vendor/repo_hotel:default",
                "@repo_hotel//:sip_hash": "workspace//vendor/repo_hotel:sip_hash",
                "@repo_india": None,
                "@repo_tango//pkg/platform:protobuf": "workspace//vendor/repo_tango/pkg/platform:protobuf",
            }
        "#};
        assert_eq!(run(source), expected);
    }

    #[test]
    fn test_multiline_last_entry_without_trailing_comma_gets_comma_when_moved() {
        // Regression: the last entry may legally omit its trailing comma. When
        // sorting moves it ahead of another entry, a comma must be inserted so
        // the entries don't splice together into unparseable source.
        let source = indoc! {r#"
            X = {
                "z": [1],
                "a": None
            }
        "#};
        let expected = indoc! {r#"
            X = {
                "a": None,
                "z": [1],
            }
        "#};
        assert_eq!(run(source), expected);
        // Idempotent: the inserted comma is not re-edited on a second pass.
        assert_eq!(run(expected), expected);
    }

    #[test]
    fn test_last_entry_value_ending_in_paren_gets_comma_after_paren() {
        // The relocated entry's value ends in `)`. The comma must land after the
        // `)` (its AST range ends past it), never inside as `foo(,)`.
        let source = indoc! {r#"
            X = {
                "z": [1],
                "a": foo()
            }
        "#};
        let expected = indoc! {r#"
            X = {
                "a": foo(),
                "z": [1],
            }
        "#};
        assert_eq!(run(source), expected);
        assert_eq!(run(expected), expected);
    }

    #[test]
    fn test_last_entry_value_ending_in_bracket_keeps_inner_commas() {
        // The relocated value `[10, 20]` contains its own commas; only one
        // trailing comma is appended after `]`, the inner commas are untouched.
        let source = indoc! {r#"
            X = {
                "z": 1,
                "a": [10, 20]
            }
        "#};
        let expected = indoc! {r#"
            X = {
                "a": [10, 20],
                "z": 1,
            }
        "#};
        assert_eq!(run(source), expected);
        assert_eq!(run(expected), expected);
    }

    #[test]
    fn test_select_default_none_without_trailing_comma() {
        // Mirrors the reported BUCKFORMAT crash: a select() dict with a `None`
        // value and no trailing comma on the relocated entry.
        let source = indoc! {r#"
            exec_compatible_with = select({
                "ovr_config//cpu/constraints:arm64": ["ovr_config//cpu/constraints:arm64"],
                "DEFAULT": None
            })
        "#};
        let expected = indoc! {r#"
            exec_compatible_with = select({
                "DEFAULT": None,
                "ovr_config//cpu/constraints:arm64": ["ovr_config//cpu/constraints:arm64"],
            })
        "#};
        assert_eq!(run(source), expected);
        assert_eq!(run(expected), expected);
    }

    #[test]
    fn test_multiline_three_entries_with_comments() {
        let source = indoc! {r#"
            x = {
                "z": 3,  # third
                "a": 1,  # first
                "m": 2,  # second
            }
        "#};
        let expected = indoc! {r#"
            x = {
                "a": 1,  # first
                "m": 2,  # second
                "z": 3,  # third
            }
        "#};
        assert_eq!(run(source), expected);
    }
}
