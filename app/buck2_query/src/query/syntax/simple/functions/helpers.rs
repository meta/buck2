/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Provides utilities for defining query functions.
//!
//! To allow a new type to be used as an argument, implement the [QueryFunctionArg] trait for that type.

use std::borrow::Cow;

use async_trait::async_trait;
use buck2_query_parser::Expr;
use buck2_query_parser::SpannedExpr;
use buck2_query_parser::spanned::Spanned;
use enum_iterator::Sequence;
use variant_name::VariantName;

use crate::query::environment::QueryEnvironment;
use crate::query::syntax::simple::eval::error::QueryError;
use crate::query::syntax::simple::eval::evaluator::QueryEvaluator;
use crate::query::syntax::simple::eval::file_set::FileSet;
use crate::query::syntax::simple::eval::set::TargetSet;
use crate::query::syntax::simple::eval::values::QueryValue;
use crate::query::syntax::simple::eval::values::QueryValueDepth;
use crate::query::syntax::simple::eval::values::QueryValueSet;

mod _scoped_allow {
    use dupe::Dupe;

    use super::*;

    #[derive(Debug, Clone, Dupe, Copy, Sequence)]
    pub enum QueryArgType {
        String,
        Integer,
        TargetSet,
        FileSet,
        Set,
        Expression,
        Value,
    }
}

pub use _scoped_allow::*;
use buck2_query::query::syntax::simple::functions::docs::MarkdownOptions;

impl QueryArgType {
    pub fn repr(self) -> &'static str {
        match self {
            QueryArgType::String => "string",
            QueryArgType::Integer => "integer",
            QueryArgType::TargetSet => "target expression",
            QueryArgType::FileSet => "file expression",
            QueryArgType::Set => "target or file expression",
            QueryArgType::Expression => "query expression",
            QueryArgType::Value => "any value",
        }
    }

    pub fn rendered_reference(self, options: &MarkdownOptions) -> String {
        let repr = format!("*{}*", self.repr());
        if options.links_enabled {
            format!("[{}](#{})", repr, self.internal_link_id())
        } else {
            repr
        }
    }

    /// Hashtag link ID to be used in Markdown
    pub fn internal_link_id(self) -> String {
        self.repr().to_lowercase().replace(" ", "-")
    }

    pub fn short_description(self, options: &MarkdownOptions) -> Option<Cow<'static, str>> {
        match self {
            QueryArgType::String => Some(Cow::Borrowed(
                "for example, `non_quoted_string` or `\"quoted string\"`",
            )),
            QueryArgType::Integer => Some(Cow::Borrowed("must be positive and fit in `u32`")),
            QueryArgType::TargetSet => Some(Cow::Borrowed(
                "either a literal or the return value of a function",
            )),
            QueryArgType::FileSet => Some(Cow::Borrowed(
                "either a literal or the return value of a function",
            )),
            QueryArgType::Expression => Some(Cow::Borrowed(
                "a valid query expression, evaluated in a function-specific context",
            )),
            QueryArgType::Value => Some(Cow::Borrowed("any query value")),
            QueryArgType::Set => Some(Cow::Owned(format!(
                "either a {} or {}",
                QueryArgType::FileSet.rendered_reference(options),
                QueryArgType::TargetSet.rendered_reference(options)
            ))),
        }
    }

    pub fn description(self) -> Option<&'static str> {
        match self {
            QueryArgType::String => None,
            QueryArgType::Integer => None,
            QueryArgType::TargetSet => Some(
                "This could be a literal build target (`\"cell//some:target\"`) or a pattern \
                (`\"cell//package:\"` or `\"cell//recursive/...\"`) or the result of another function that returns a target expression. For \
                queries in CLI commands (like `buck2 query`), literals can be relative to the current working dir (like `some:target` \
                or `...`).",
            ),
            QueryArgType::FileSet => Some(
                "This could be a file literal like `path/to/a.file` or the return value of a function that \
                returns files (for example, the `buildfile()` function).",
            ),
            QueryArgType::Set => Some(
                "This could be a literal like `path/to/a.file` or `\"cell//some:target\"`, \
                or the return value of a function that returns files or targets.",
            ),
            QueryArgType::Expression => Some(
                "This is used for functions that capture an expression and evaluate it in another context. \
                For example, the `deps()` function can accept an expression that it uses to find the children of a node to \
                customize the deps traversal.",
            ),
            _ => None,
        }
    }
}

#[async_trait]
pub trait QueryFunction<Env: QueryEnvironment>: Send + Sync {
    fn name(&self) -> &'static str;

    async fn invoke(
        &self,
        evaluator: &QueryEvaluator<Env>,
        args: &[SpannedExpr<'_>],
    ) -> Result<QueryValue<Env::Target>, QueryError>;

    fn arg_type(&self, idx: usize) -> Result<QueryArgType, QueryError>;
    fn arg_name(&self, idx: usize) -> Result<&'static str, QueryError>;
}

#[async_trait]
pub trait QueryBinaryOp<Env: QueryEnvironment>: Send + Sync {
    fn name(&self) -> &'static str;

    async fn invoke(
        &self,
        env: &Env,
        left: QueryValue<Env::Target>,
        right: QueryValue<Env::Target>,
    ) -> Result<QueryValue<Env::Target>, QueryError>;
}

#[async_trait]
pub trait QueryFunctionArg<'a, Env: QueryEnvironment>: Sized + Send + Sync {
    const ARG_TYPE: QueryArgType;

    fn describe_format() -> String {
        "{}".to_owned()
    }

    /// accept_none() is called when no arg is provided. This is used for optional args.
    fn accept_none() -> Option<Self> {
        None
    }

    async fn eval(
        evaluator: &QueryEvaluator<'a, Env>,
        expr: &'a Spanned<Expr<'a>>,
    ) -> Result<Self, QueryError> {
        let result = evaluator.eval(expr).await?;
        Self::accept(evaluator.env(), result.value).await
    }

    /// accept() will be called with the value of the evaluated expression for an arg. It
    /// should perform type checking and return the typed value or an error.
    async fn accept(env: &Env, val: QueryValue<Env::Target>) -> Result<Self, QueryError>;
}

pub struct CapturedExpr<'a> {
    pub expr: &'a Spanned<Expr<'a>>,
}

/// Allows a "captured expression" as a function argument. This is used for a function like
/// `deps()` that uses an argument similar to a lambda where it will evaluate the expression
/// itself in some other context.
#[async_trait]
impl<'a, Env: QueryEnvironment> QueryFunctionArg<'a, Env> for CapturedExpr<'a> {
    const ARG_TYPE: QueryArgType = QueryArgType::Expression;

    async fn eval(
        _evaluator: &QueryEvaluator<'a, Env>,
        expr: &'a Spanned<Expr<'a>>,
    ) -> Result<Self, QueryError> {
        Ok(CapturedExpr { expr })
    }

    async fn accept(_env: &Env, _val: QueryValue<Env::Target>) -> Result<Self, QueryError> {
        unimplemented!("accept shouldn't be called since eval doesn't use it")
    }
}

#[async_trait]
impl<'a, Env: QueryEnvironment> QueryFunctionArg<'a, Env> for QueryValue<Env::Target> {
    const ARG_TYPE: QueryArgType = QueryArgType::Value;

    async fn accept(_env: &Env, val: QueryValue<Env::Target>) -> Result<Self, QueryError> {
        Ok(val)
    }
}

#[async_trait]
impl<'a, Env: QueryEnvironment> QueryFunctionArg<'a, Env> for QueryValueSet<Env::Target> {
    const ARG_TYPE: QueryArgType = QueryArgType::Set;

    async fn accept(env: &Env, val: QueryValue<Env::Target>) -> Result<Self, QueryError> {
        match val {
            QueryValue::String(s) => Ok(QueryValueSet::TargetSet(env.eval_literals(&[&s]).await?)),
            QueryValue::TargetSet(x) => Ok(QueryValueSet::TargetSet(x)),
            QueryValue::FileSet(x) => Ok(QueryValueSet::FileSet(x)),
            _ => Err(QueryError::InvalidType {
                expected: "file or target set",
                actual: val.variant_name(),
            }),
        }
    }
}

/// Straightforward implementation for String.
#[async_trait]
impl<'a, Env: QueryEnvironment> QueryFunctionArg<'a, Env> for String {
    const ARG_TYPE: QueryArgType = QueryArgType::String;

    async fn accept(_env: &Env, val: QueryValue<Env::Target>) -> Result<Self, QueryError> {
        match val {
            QueryValue::String(s) => Ok(s),
            _ => Err(QueryError::InvalidType {
                expected: "string",
                actual: val.variant_name(),
            }),
        }
    }
}

/// Straightforward implementation for TargetSet.
#[async_trait]
impl<'a, Env: QueryEnvironment> QueryFunctionArg<'a, Env> for TargetSet<Env::Target> {
    const ARG_TYPE: QueryArgType = QueryArgType::TargetSet;

    async fn accept(env: &Env, val: QueryValue<Env::Target>) -> Result<Self, QueryError> {
        match val {
            QueryValue::String(s) => Ok(env.eval_literals(&[&s]).await?),
            QueryValue::TargetSet(t) => Ok(t),
            _ => Err(QueryError::InvalidType {
                expected: "target_set",
                actual: val.variant_name(),
            }),
        }
    }
}

/// Straightforward implementation for FileSet.
#[async_trait]
impl<'a, Env: QueryEnvironment> QueryFunctionArg<'a, Env> for FileSet {
    const ARG_TYPE: QueryArgType = QueryArgType::FileSet;

    async fn accept(env: &Env, val: QueryValue<Env::Target>) -> Result<Self, QueryError> {
        match val {
            QueryValue::String(v) => Ok(env.eval_file_literal(&v).await?),
            QueryValue::FileSet(t) => Ok(t),
            _ => Err(QueryError::InvalidType {
                expected: "target_set",
                actual: val.variant_name(),
            }),
        }
    }
}

/// Used to specify the depth of some graph traversals. None maps to unbounded,
/// and u64 is clamped to a u32 and converted such that large values
/// automatically become unbounded (see `QueryValueDepth`).
#[async_trait]
impl<'a, Env: QueryEnvironment> QueryFunctionArg<'a, Env> for QueryValueDepth {
    const ARG_TYPE: QueryArgType = QueryArgType::Integer;

    fn accept_none() -> Option<Self> {
        Some(QueryValueDepth::Unbounded)
    }

    async fn accept(_env: &Env, val: QueryValue<Env::Target>) -> Result<Self, QueryError> {
        match val {
            QueryValue::Integer(v) => Ok((v.clamp(0, u32::MAX as u64) as u32).into()),
            _ => Err(QueryError::InvalidType {
                expected: "uint",
                actual: val.variant_name(),
            }),
        }
    }
}

/// Straightforward implementation for u64.
#[async_trait]
impl<'a, Env: QueryEnvironment> QueryFunctionArg<'a, Env> for u64 {
    const ARG_TYPE: QueryArgType = QueryArgType::Integer;

    async fn accept(_env: &Env, val: QueryValue<Env::Target>) -> Result<Self, QueryError> {
        match val {
            QueryValue::Integer(v) => Ok(v),
            _ => Err(QueryError::InvalidType {
                expected: "uint",
                actual: val.variant_name(),
            }),
        }
    }
}

/// `Option<T>` is used for optional args and so this is the (only?) implementation that supports a "none" arg.
#[async_trait]
impl<'a, Env: QueryEnvironment, A: QueryFunctionArg<'a, Env>> QueryFunctionArg<'a, Env>
    for Option<A>
{
    const ARG_TYPE: QueryArgType = A::ARG_TYPE;

    fn accept_none() -> Option<Self> {
        Some(None)
    }

    async fn accept(env: &Env, val: QueryValue<Env::Target>) -> Result<Self, QueryError> {
        Ok(Some(A::accept(env, val).await?))
    }

    async fn eval(
        evaluator: &QueryEvaluator<'a, Env>,
        expr: &'a Spanned<Expr<'a>>,
    ) -> Result<Self, QueryError> {
        Ok(Some(A::eval(evaluator, expr).await?))
    }

    fn describe_format() -> String {
        "?{}".to_owned()
    }
}

// Helper for buck_query_proc_macro implementations. Evaluates an arg at an index and tries to convert it to the QueryFunctionArg type, providing decent errors on failures.
pub async fn eval_arg<'a, Env: QueryEnvironment, A: QueryFunctionArg<'a, Env>>(
    func: &impl QueryFunction<Env>,
    evaluator: &QueryEvaluator<'a, Env>,
    args: &'a [Spanned<Expr<'a>>],
    idx: usize,
) -> Result<A, QueryError> {
    match args.get(idx) {
        Some(v) => {
            if let Expr::None = v.value {
                match A::accept_none() {
                    Some(v) => Ok(v),
                    None => Err(QueryError::NoneNotAccepted {
                        function: func.name().to_owned(),
                        arg_idx: idx.to_string(),
                        arg_name: func.arg_name(idx)?.to_owned(),
                        arg_type: A::ARG_TYPE.rendered_reference(&MarkdownOptions {
                            links_enabled: false,
                        }),
                    }),
                }
            } else {
                A::eval(evaluator, v).await
            }
        }
        None => match A::accept_none() {
            Some(v) => Ok(v),
            None => Err(QueryError::TooFewArgs {
                function: func.name().to_owned(),
                next_arg_name: func.arg_name(idx)?.to_owned(),
                min: idx + 1,
                actual: args.len(),
            }),
        },
    }
}
