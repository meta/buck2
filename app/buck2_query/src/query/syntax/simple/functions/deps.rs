/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Debug;
use std::marker::PhantomData;

use async_trait::async_trait;
use buck2_query_derive::query_module;
use variant_name::VariantName;

use crate::query::environment::QueryEnvironment;
use crate::query::environment::QueryTarget;
use crate::query::environment::TraversalFilter;
use crate::query::syntax::simple::eval::error::QueryError;
use crate::query::syntax::simple::eval::evaluator::QueryEvaluator;
use crate::query::syntax::simple::eval::set::TargetSet;
use crate::query::syntax::simple::eval::values::QueryEvaluationValue;
use crate::query::syntax::simple::eval::values::QueryValue;
use crate::query::syntax::simple::eval::values::QueryValueDepth;
use crate::query::syntax::simple::functions::AugmentedQueryFunctions;
use crate::query::syntax::simple::functions::QueryFunctions;
use crate::query::syntax::simple::functions::helpers::CapturedExpr;

pub(crate) struct DepsContextFunctions<'a, Env: QueryEnvironment> {
    target: &'a Env::Target,
}

impl<'a, Env: QueryEnvironment> Debug for DepsContextFunctions<'a, Env> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DepsContextFunctions")
            .finish_non_exhaustive()
    }
}

#[query_module(Env)]
impl<'a, Env: QueryEnvironment> DepsContextFunctions<'a, Env> {
    async fn first_order_deps(&self, env: &Env) -> Result<QueryValue<Env::Target>, QueryError> {
        let mut deps = TargetSet::new();
        for dep in self.target.deps() {
            deps.insert(env.get_node(dep).await?);
        }
        Ok(QueryValue::TargetSet(deps))
    }

    async fn exec_deps(&self, env: &Env) -> Result<QueryValue<Env::Target>, QueryError> {
        let mut deps = TargetSet::new();
        for dep in self.target.exec_deps() {
            deps.insert(env.get_node(dep).await?);
        }
        Ok(QueryValue::TargetSet(deps))
    }

    async fn target_deps(&self, env: &Env) -> Result<QueryValue<Env::Target>, QueryError> {
        let mut deps = TargetSet::new();
        for dep in self.target.target_deps() {
            deps.insert(env.get_node(dep).await?);
        }
        Ok(QueryValue::TargetSet(deps))
    }

    async fn configuration_deps(&self, env: &Env) -> Result<QueryValue<Env::Target>, QueryError> {
        let mut deps = TargetSet::new();
        for dep in self.target.configuration_deps() {
            deps.insert(env.get_node(dep).await?);
        }
        Ok(QueryValue::TargetSet(deps))
    }

    async fn toolchain_deps(&self, env: &Env) -> Result<QueryValue<Env::Target>, QueryError> {
        let mut deps = TargetSet::new();
        for dep in self.target.toolchain_deps() {
            deps.insert(env.get_node(dep).await?);
        }
        Ok(QueryValue::TargetSet(deps))
    }
}

pub(crate) struct DepsFunction<Env: QueryEnvironment> {
    pub(crate) _marker: PhantomData<Env>,
}

struct Filter<'a, Env: QueryEnvironment> {
    inner_env: &'a Env,
    functions: &'a dyn QueryFunctions<Env = Env>,
    expr: &'a CapturedExpr<'a>,
}

impl<'a, Env: QueryEnvironment> DepsFunction<Env> {
    fn make_filter(
        &'a self,
        env: &'a Env,
        functions: &'a dyn QueryFunctions<Env = Env>,
        captured_expr: Option<&'a CapturedExpr>,
    ) -> Option<Filter<'a, Env>> {
        match captured_expr {
            Some(expr) => {
                #[async_trait]
                #[allow(non_local_definitions)]
                impl<'a, T: QueryTarget, Env: QueryEnvironment<Target = T>> TraversalFilter<T> for Filter<'a, Env> {
                    async fn get_children(&self, target: &T) -> buck2_error::Result<TargetSet<T>> {
                        let augmented_functions = AugmentedQueryFunctions::augment(
                            self.functions,
                            Box::new(DepsContextFunctions { target }),
                        );
                        let evaluator = QueryEvaluator::new(self.inner_env, &augmented_functions);
                        match evaluator.eval_parsed_query(self.expr.expr).await {
                            Ok(v) => match v.value {
                                QueryEvaluationValue::TargetSet(v) => Ok(v),
                                v => Err(QueryError::InvalidType {
                                    expected: "targets",
                                    actual: v.variant_name(),
                                }
                                .into()),
                            },
                            Err(e) => Err(QueryError::drop_spans(e)),
                        }
                    }
                }

                Some(Filter::<'a, Env> {
                    inner_env: env,
                    functions,
                    expr,
                })
            }
            None => None,
        }
    }

    pub(crate) async fn invoke_deps(
        &self,
        env: &Env,
        functions: &dyn QueryFunctions<Env = Env>,
        targets: &TargetSet<Env::Target>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr<'_>>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        let filter = self.make_filter(env, functions, captured_expr);
        let filter_ref = filter
            .as_ref()
            .map(|v| v as &dyn TraversalFilter<Env::Target>);

        env.deps(targets, depth, filter_ref).await
    }

    pub(crate) async fn invoke_rdeps(
        &self,
        env: &Env,
        functions: &dyn QueryFunctions<Env = Env>,
        universe: &TargetSet<Env::Target>,
        from: &TargetSet<Env::Target>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr<'_>>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        let filter = self.make_filter(env, functions, captured_expr);
        let filter_ref = filter
            .as_ref()
            .map(|v| v as &dyn TraversalFilter<Env::Target>);

        env.rdeps(universe, from, depth, filter_ref).await
    }

    pub(crate) async fn invoke_somepath(
        &self,
        env: &Env,
        functions: &dyn QueryFunctions<Env = Env>,
        from: &TargetSet<Env::Target>,
        to: &TargetSet<Env::Target>,
        captured_expr: Option<&CapturedExpr<'_>>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        let filter = self.make_filter(env, functions, captured_expr);
        let filter_ref = filter
            .as_ref()
            .map(|v| v as &dyn TraversalFilter<Env::Target>);

        env.somepath(from, to, filter_ref).await
    }

    pub(crate) async fn invoke_allpaths(
        &self,
        env: &Env,
        functions: &dyn QueryFunctions<Env = Env>,
        from: &TargetSet<Env::Target>,
        to: &TargetSet<Env::Target>,
        captured_expr: Option<&CapturedExpr<'_>>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        let filter = self.make_filter(env, functions, captured_expr);
        let filter_ref = filter
            .as_ref()
            .map(|v| v as &dyn TraversalFilter<Env::Target>);

        env.allpaths(from, to, filter_ref).await
    }
}
