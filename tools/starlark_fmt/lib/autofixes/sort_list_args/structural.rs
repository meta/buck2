/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashSet;

use anyhow::Context as _;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprList;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::visitor::walk_expr;
use ruff_python_ast::visitor::walk_stmt;
use ruff_text_size::Ranged;
use ruff_text_size::TextRange;

use super::ElementSortContext;
use super::ListArgSorter;
use super::call_func_name;
use super::collect_sorted_elements_with_keys;
use super::has_do_not_sort_directive;
use super::has_do_not_sort_on_first_element;
use super::has_keep_sorted_on_first_element;
use super::has_unsafe_mixed_line_comment_layout;
use super::policy;
use super::policy::leading_comments_before;
use crate::autofixes::fmt_suppression::find_fmt_off_ranges;
use crate::autofixes::fmt_suppression::overlaps_fmt_off_region;
use crate::autofixes::parsed_module::Edit;
use crate::autofixes::parsed_module::ParsedModule;
use crate::autofixes::parsed_module::format_location;
use crate::config::Config;
use crate::sort_key::SortKey;
use crate::sort_key::call_name as full_call_name;

#[derive(Clone, Copy)]
enum SortKeySource {
    Configured,
    Inline,
}

struct StructuralListArgSorter<'a, 'c> {
    legacy: ListArgSorter<'a, 'c>,
    lists_with_edits: HashSet<TextRange>,
    nested_work_deferred: bool,
    fmt_off_ranges: Vec<TextRange>,
    has_inline_directives: bool,
    error: Option<anyhow::Error>,
}

impl<'a, 'c> StructuralListArgSorter<'a, 'c> {
    fn new(
        module: &'a ParsedModule<'a>,
        config: &'c Config,
        sort_rule_args: bool,
        has_inline_directives: bool,
    ) -> Self {
        Self {
            legacy: ListArgSorter::new(module, config, sort_rule_args),
            lists_with_edits: HashSet::new(),
            nested_work_deferred: false,
            fmt_off_ranges: find_fmt_off_ranges(module.source(), module.line_index()),
            has_inline_directives,
            error: None,
        }
    }

    fn sort_list_with_key(
        &mut self,
        list: &ExprList,
        key: &SortKey,
        source: SortKeySource,
    ) -> anyhow::Result<()> {
        if !self.legacy.handled_lists.insert(list.range())
            || has_do_not_sort_on_first_element(self.legacy.module, list)
        {
            return Ok(());
        }
        // Single-element lists are already sorted; skip key extraction and
        // allocation entirely (common for `visibility = ["//visibility:public"]`).
        // Configured keys fail open, so skipping is safe. Inline directives
        // fail closed: a single element must still be validated so an
        // unmatched key surfaces instead of silently passing.
        if list.elts.len() < 2 && matches!(source, SortKeySource::Configured) {
            return Ok(());
        }

        // Unsafe layouts fail open for configured keys (skip the list) but
        // fail closed for inline directives (report the error). Aborting the
        // whole module here would discard unrelated edits already collected.
        if has_unsafe_mixed_line_comment_layout(list, self.legacy.module) {
            match source {
                SortKeySource::Configured => return Ok(()),
                SortKeySource::Inline => anyhow::bail!(
                    "{}: cannot safely sort a list with comments and multiple elements on one line",
                    format_location(self.legacy.module.source(), list.range())
                ),
            }
        }

        let mut sort_keys = Vec::with_capacity(list.elts.len());
        for element in &list.elts {
            match (source, key.extract(element)) {
                (_, Ok(Some(value))) => sort_keys.push(Some(value)),
                (SortKeySource::Configured, Ok(None) | Err(_)) => return Ok(()),
                (SortKeySource::Inline, Ok(None)) => anyhow::bail!(
                    "{}: no inline sort key matched this list element",
                    format_location(self.legacy.module.source(), element.range())
                ),
                (SortKeySource::Inline, Err(error)) => {
                    return Err(error).with_context(|| {
                        format!(
                            "{}: failed to extract an inline sort key",
                            format_location(self.legacy.module.source(), element.range())
                        )
                    });
                }
            }
        }

        let context = ElementSortContext {
            list,
            module: self.legacy.module,
            sort_keys: &sort_keys,
            deduplicate: false,
        };
        if let Some(element_edits) = collect_sorted_elements_with_keys(&context, true) {
            self.lists_with_edits.insert(list.range());
            // Edited lists are not descended into, so nested sortable work
            // inside them needs a second pass to be discovered.
            if !self.nested_work_deferred && list_contains_nested_work(list) {
                self.nested_work_deferred = true;
            }
            self.legacy.edits.extend(
                element_edits
                    .into_iter()
                    .map(|(range, replacement)| Edit::new(range, replacement)),
            );
        }
        Ok(())
    }

    fn sort_lists_in_expr_with_key(
        &mut self,
        expr: &Expr,
        key: &SortKey,
        source: SortKeySource,
    ) -> anyhow::Result<usize> {
        match expr {
            Expr::List(list) => {
                self.sort_list_with_key(list, key, source)?;
                Ok(1)
            }
            Expr::BinOp(binop) if matches!(binop.op, Operator::Add) => Ok(self
                .sort_lists_in_expr_with_key(&binop.left, key, source)?
                + self.sort_lists_in_expr_with_key(&binop.right, key, source)?),
            Expr::Call(call) if call_func_name(call) == Some("select") => {
                let Some(Expr::Dict(dict)) = call.arguments.args.first() else {
                    return Ok(0);
                };
                dict.items.iter().try_fold(0, |count, item| {
                    Ok(count + self.sort_lists_in_expr_with_key(&item.value, key, source)?)
                })
            }
            _ => Ok(0),
        }
    }
}

impl<'a, 'c> Visitor<'a> for StructuralListArgSorter<'a, 'c> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let Stmt::Assign(assign) = stmt
            && let Expr::List(list) = assign.value.as_ref()
            && self.legacy.has_keep_sorted_before_stmt(stmt)
        {
            self.legacy.sort_list(list, true);
        }

        if let Stmt::AnnAssign(assign) = stmt
            && let Some(value) = &assign.value
            && let Expr::List(list) = value.as_ref()
            && self.legacy.has_keep_sorted_before_stmt(stmt)
        {
            self.legacy.sort_list(list, true);
        }

        walk_stmt(self, stmt);
        self.legacy.prev_stmt_end = stmt.range().end();
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.error.is_some() {
            return;
        }

        match expr {
            Expr::Call(call) => {
                self.legacy.call_stack.push(call_func_name(call));
                let mut configured_callee = None;

                for keyword in &call.arguments.keywords {
                    if overlaps_fmt_off_region(keyword.range(), &self.fmt_off_ranges) {
                        continue;
                    }
                    // Cheap gate before the line-walking comment lookups
                    // below: skip keywords that nothing will sort. This must
                    // stay in sync with the match arms below — it reuses
                    // their exact inputs (`configured_key` is shared, and
                    // `legacy_will_sort` is reused by the legacy arm), so a
                    // new sorting path added below must extend this gate.
                    // Resolution itself is map lookups; the callee name is
                    // computed once per call and cached in
                    // `configured_callee`.
                    let configured_key = keyword.arg.as_ref().and_then(|arg| {
                        self.legacy
                            .config
                            .list_sort_keys()
                            .and_then(|sort_keys| sort_keys.for_arg(arg.as_str()))
                            .and_then(|sort_keys| {
                                let callee = sort_keys.has_callee_keys().then(|| {
                                    configured_callee
                                        .get_or_insert_with(|| full_call_name(&call.func))
                                        .as_deref()
                                });
                                sort_keys.resolve(callee.flatten())
                            })
                    });
                    let legacy_will_sort = keyword.arg.as_ref().is_some_and(|arg| {
                        self.legacy.sort_rule_args && self.legacy.should_sort_arg(arg.as_str())
                    });
                    if configured_key.is_none() && !legacy_will_sort && !self.has_inline_directives
                    {
                        continue;
                    }
                    let leading_comments =
                        leading_comments_before(self.legacy.module, keyword.range().start());
                    if leading_comments
                        .is_some_and(|range| has_do_not_sort_directive(self.legacy.module, range))
                        || has_do_not_sort_directive(self.legacy.module, keyword.range())
                    {
                        continue;
                    }

                    let inline_sort_key = if self.has_inline_directives {
                        policy::inline_sort_key_before(self.legacy.module, keyword.range().start())
                    } else {
                        None
                    };
                    if inline_sort_key.is_none()
                        && self.has_inline_directives
                        && self.error.is_none()
                        && let Some(directive) = policy::unattached_inline_directive_above(
                            self.legacy.module,
                            keyword.range().start(),
                        )
                    {
                        self.error = Some(anyhow::anyhow!(
                            "{}: inline sort directive must be directly above its keyword argument",
                            format_location(self.legacy.module.source(), directive)
                        ));
                    }
                    match inline_sort_key {
                        Some(inline) => {
                            let result =
                                inline.parse(self.legacy.module.source()).and_then(|key| {
                                    self.sort_lists_in_expr_with_key(
                                        &keyword.value,
                                        &key,
                                        SortKeySource::Inline,
                                    )
                                });
                            match result {
                                Ok(0) => {
                                    self.error = Some(anyhow::anyhow!(
                                        "{}: inline sort directive does not apply to a supported list expression",
                                        format_location(
                                            self.legacy.module.source(),
                                            keyword.range()
                                        )
                                    ));
                                }
                                Ok(_) => {}
                                Err(error) => self.error = Some(error),
                            }
                        }
                        None => {
                            let Some(arg_name) = &keyword.arg else {
                                continue;
                            };

                            if let Some(key) = configured_key
                                && !self.legacy.is_arg_blocklisted(arg_name.as_str())
                            {
                                if let Err(error) = self.sort_lists_in_expr_with_key(
                                    &keyword.value,
                                    key,
                                    SortKeySource::Configured,
                                ) {
                                    self.error = Some(error);
                                }
                            } else if legacy_will_sort {
                                self.legacy.sort_lists_in_expr(&keyword.value);
                            }
                        }
                    }

                    if self.error.is_some() {
                        break;
                    }
                }

                walk_expr(self, expr);
                self.legacy.call_stack.pop();
            }
            Expr::List(list) => {
                if self.lists_with_edits.contains(&list.range()) {
                    return;
                }
                if has_keep_sorted_on_first_element(self.legacy.module, list) {
                    self.legacy.sort_list(list, true);
                }
                walk_expr(self, expr);
            }
            _ => walk_expr(self, expr),
        }
    }
}

/// Probe for nested lists or calls inside an edited list.
struct NestedWorkProbe {
    found: bool,
}

impl<'a> Visitor<'a> for NestedWorkProbe {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.found {
            return;
        }
        match expr {
            Expr::List(_) | Expr::Call(_) => self.found = true,
            _ => walk_expr(self, expr),
        }
    }
}

/// True when `list` holds a nested list or call. All sortable work lives
/// under one of those nodes, and edited lists are skipped during the visit
/// (see the `Expr::List` arm), so only such lists need a second pass for
/// their nested work to be discovered. Flat lists converge in one pass.
fn list_contains_nested_work(list: &ExprList) -> bool {
    let mut probe = NestedWorkProbe { found: false };
    for element in &list.elts {
        probe.visit_expr(element);
        if probe.found {
            return true;
        }
    }
    false
}

struct ListEditBatch {
    edits: Vec<Edit>,
    deferred_descendants: bool,
    fmt_off_ranges: Vec<TextRange>,
}

fn collect_edits(
    module: &ParsedModule,
    config: &Config,
    sort_rule_args: bool,
    has_inline_directives: bool,
) -> anyhow::Result<ListEditBatch> {
    let mut sorter =
        StructuralListArgSorter::new(module, config, sort_rule_args, has_inline_directives);
    for stmt in module.stmts() {
        sorter.visit_stmt(stmt);
    }

    match sorter.error {
        Some(error) => Err(error),
        None => Ok(ListEditBatch {
            edits: sorter.legacy.edits,
            deferred_descendants: sorter.nested_work_deferred,
            fmt_off_ranges: sorter.fmt_off_ranges,
        }),
    }
}

pub(super) fn apply<'a>(
    mut module: ParsedModule<'a>,
    config: &Config,
    sort_rule_args: bool,
    has_inline_directives: bool,
) -> anyhow::Result<ParsedModule<'a>> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash as _;
    use std::hash::Hasher as _;

    // Track visited sources by hash to detect non-convergence without
    // cloning the entire file contents on every pass.
    let mut seen = HashSet::new();
    loop {
        let ListEditBatch {
            edits,
            deferred_descendants,
            fmt_off_ranges,
        } = collect_edits(&module, config, sort_rule_args, has_inline_directives)?;
        let (next, changed) =
            module.run_transform_checked_with_fmt_off_ranges(&fmt_off_ranges, |_| edits)?;
        module = next;

        if !changed || !deferred_descendants {
            return Ok(module);
        }
        let mut hasher = DefaultHasher::new();
        module.source().hash(&mut hasher);
        if !seen.insert(hasher.finish()) {
            anyhow::bail!("custom list sorting did not converge");
        }
    }
}
