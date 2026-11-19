/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Implementation of the cli and query_* attr query language.

use buck2_hash::BuckMutInsertionSet;
use buck2_query_parser::Expr;
use buck2_query_parser::parse_expr;
use buck2_query_parser::spanned::Spanned;
use futures::FutureExt;
use variant_name::VariantName;

use crate::query::environment::QueryEnvironment;
use crate::query::syntax::simple::eval::error::QueryError;
use crate::query::syntax::simple::eval::file_set::FileNode;
use crate::query::syntax::simple::eval::file_set::FileSet;
use crate::query::syntax::simple::eval::set::TargetSet;
use crate::query::syntax::simple::eval::values::QueryEvaluationValue;
use crate::query::syntax::simple::eval::values::QueryResult;
use crate::query::syntax::simple::eval::values::QueryValue;
use crate::query::syntax::simple::functions::QueryFunctions;
pub struct QueryEvaluator<'e, Env: QueryEnvironment> {
    env: &'e Env,
    functions: &'e dyn QueryFunctions<Env = Env>,
}

impl<'e, Env: QueryEnvironment> QueryEvaluator<'e, Env> {
    pub fn new(env: &'e Env, functions: &'e dyn QueryFunctions<Env = Env>) -> Self {
        Self { env, functions }
    }

    pub fn env(&self) -> &Env {
        self.env
    }

    pub fn functions(&self) -> &dyn QueryFunctions<Env = Env> {
        self.functions
    }

    async fn resolve_literal(&self, literal: &str) -> buck2_error::Result<TargetSet<Env::Target>> {
        self.env.eval_literals(&[literal]).await
    }

    async fn eval_internal(&self, expr: &Expr<'_>) -> Result<QueryValue<Env::Target>, QueryError> {
        // TODO(cjhopman): We should extract these functions to a map of name->functionobj and attach
        // more information to them like documentation and signature. Potentially we could generalize
        // the function impls there to work across bxl and here, but not sure if that's worth the
        // complexity or not.
        match expr {
            Expr::None => Ok(QueryValue::None),
            Expr::String(word) => Ok(QueryValue::String((*word).to_owned())),
            Expr::Integer(value) => Ok(QueryValue::Integer(*value)),
            Expr::Function {
                function_name,
                args,
            } => match self.functions.get(function_name) {
                Some(func) => func.invoke(self, args).await,
                None => Err(QueryError::UnknownFunction(
                    (*function_name.fragment()).to_owned(),
                )),
            },
            Expr::BinaryOpSequence(left, exprs) => {
                let (left, rights) = futures::future::try_join(
                    self.eval(left),
                    buck2_util::future::try_join_all(exprs.iter().map(|(op, expr)| async move {
                        let value = self.eval(expr).await?;
                        Ok((op, value))
                    })),
                )
                .await?;
                let mut value = left.value;
                for (op, right) in rights {
                    value = right
                        .async_into_map_res(|right| async move {
                            match self.functions.get_op(*op) {
                                Some(func) => func.invoke(self.env(), value, right).await,
                                None => Err(QueryError::UnsupportedBinaryOp(op.to_string())),
                            }
                        })
                        .await?
                        .value;
                }
                Ok(value)
            }
            Expr::Set(args) => {
                let patterns: Vec<_> = args.iter().map(|v| v.fragment()).collect::<Vec<_>>();
                // TODO(cjhopman): evaluating the literals in this way does not preserver the ordering from
                // the user, instead the result will be package-ordered. We may need to change this to
                // preserve order.
                Ok(self.env.eval_literals(&patterns).await?.into())
            }
            Expr::FileSet(args) => {
                let patterns: Vec<_> = args.iter().map(|v| v.fragment()).collect::<Vec<_>>();
                let mut files = FileSet::new(BuckMutInsertionSet::<FileNode>::new());

                for pattern in patterns {
                    files.insert_all(&self.env.eval_file_literal(pattern).await?)
                }

                Ok(files.into())
            }
        }
    }

    pub fn eval<'a>(
        &'a self,
        expr: &'a Spanned<Expr<'a>>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = QueryResult<QueryValue<Env::Target>>> + Send + 'a>,
    > {
        async move { expr.span(self.eval_internal(&expr.value).await) }.boxed()
    }

    pub async fn eval_query(
        &self,
        query: &str,
    ) -> buck2_error::Result<QueryEvaluationValue<Env::Target>> {
        let parsed_query = parse_expr(query)?;
        match self.eval_parsed_query(&parsed_query).await {
            Ok(v) => Ok(v.value),
            Err(e) => Err(QueryError::convert_error(e, query)),
        }
    }

    pub async fn eval_parsed_query<'a>(
        &self,
        expr: &Spanned<Expr<'a>>,
    ) -> QueryResult<QueryEvaluationValue<Env::Target>> {
        self.eval(expr)
            .await?
            .async_into_map_res(|value| async move {
                match value {
                    // A top-level string we treat as a target pattern and resolve it. This allows something like
                    // `buck2 query //lib/...` to resolve to the corresponding targets.
                    QueryValue::String(word) => Ok(QueryEvaluationValue::TargetSet(
                        self.resolve_literal(&word).await?,
                    )),
                    QueryValue::TargetSet(targets) => Ok(QueryEvaluationValue::TargetSet(targets)),
                    QueryValue::FileSet(files) => Ok(QueryEvaluationValue::FileSet(files)),
                    _ => Err(QueryError::InvalidType {
                        expected: "targets",
                        actual: value.variant_name(),
                    }),
                }
            })
            .await
    }
}
