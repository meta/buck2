/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;

use buck2_core::deprecation;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use buck2_error::BuckErrorOptionContext;
use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::attr_type::query::QueryAttrBase;
use buck2_node::attrs::attr_type::query::QueryAttrType;
use buck2_node::attrs::attr_type::query::ResolvedQueryLiterals;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use buck2_query::query::syntax::simple::functions::QueryLiteralVisitor;
use buck2_query_parser::parse_expr;
use starlark::typing::Ty;
use starlark::values::Value;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum QueryError {
    #[error("Query literal `{0}` names a subtarget; query evaluation resolves whole targets")]
    SubtargetInQueryLiteral(String),
}

pub trait QueryAttrTypeExt {
    fn coerce<'v>(
        ctx: &dyn AttrCoercionContext<'v>,
        query: String,
    ) -> buck2_error::Result<QueryAttrBase<ProvidersLabel>>;
}

impl QueryAttrTypeExt for QueryAttrType {
    fn coerce<'v>(
        ctx: &dyn AttrCoercionContext<'v>,
        query: String,
    ) -> buck2_error::Result<QueryAttrBase<ProvidersLabel>> {
        // parse the expr to do validation and to extract the literals.
        let parsed_query = parse_expr(&query)?;

        struct Collector<'q, 'v> {
            ctx: &'q dyn AttrCoercionContext<'v>,
            literals: BTreeMap<&'q str, ProvidersLabel>,
        }

        impl<'q, 'v> QueryLiteralVisitor<'q> for Collector<'q, 'v> {
            fn target_pattern(&mut self, pattern: &'q str) -> buck2_error::Result<()> {
                // TODO(cjhopman): We could probably parse the pattern first. This would likely at least give a better error message when the query contains a non-literal target pattern.
                // We could optimize this to do less work for duplicates, but it's generally not helpful.
                let label = self.ctx.coerce_providers_label(pattern)?;
                if label.name() != &ProvidersName::Default {
                    deprecation!(
                        "query_literal_subtarget",
                        QueryError::SubtargetInQueryLiteral(pattern.to_owned()).into(),
                        owner: "jtbraun", started: "2026-09-12",
                    );
                }
                self.literals.insert(pattern, label);
                Ok(())
            }
        }

        let mut collector = Collector {
            ctx,
            literals: BTreeMap::new(),
        };

        ctx.visit_query_function_literals(&mut collector, &parsed_query, &query)?;

        let resolved_literals = ResolvedQueryLiterals(
            collector
                .literals
                .into_iter()
                .map(|(l, p)| {
                    query
                        .as_str()
                        .substr_range(l)
                        .map(|range| ((range.start, range.end - range.start), p))
                        .internal_error("Not found in query")
                })
                .try_collect()?,
        );
        Ok(QueryAttrBase {
            query,
            resolved_literals,
        })
    }
}

impl AttrTypeCoerce for QueryAttrType {
    fn coerce_item<'v>(
        &self,
        _configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        let query = Self::coerce(ctx, value.unpack_str_err()?.to_owned())?;
        ctx.attr_arena().encode_query(&query)
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::Basic(Ty::string())
    }
}
