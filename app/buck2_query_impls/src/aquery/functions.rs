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

use buck2_build_api::actions::query::ActionQueryNode;
use buck2_build_api::actions::query::ActionQueryNodeData;
use buck2_query::query::syntax::simple::eval::error::QueryError;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_query::query::syntax::simple::eval::values::QueryValue;
use buck2_query::query::syntax::simple::functions::DefaultQueryFunctionsModule;
use buck2_query::query::syntax::simple::functions::QueryFunctions;
use buck2_query::query::syntax::simple::functions::helpers::QueryBinaryOp;
use buck2_query::query::syntax::simple::functions::helpers::QueryFunction;
use buck2_query::query_module;
use buck2_query_parser::BinaryOp;
use dupe::Dupe;

use crate::aquery::environment::AqueryEnvironment;

pub(crate) fn aquery_functions<'a>() -> impl QueryFunctions<Env = AqueryEnvironment<'a>> {
    struct Functions<'a> {
        defaults: DefaultQueryFunctionsModule<AqueryEnvironment<'a>>,
        extra_functions: AqueryFunctions<'a>,
    }

    impl Debug for Functions<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Functions").finish_non_exhaustive()
        }
    }

    impl<'a> QueryFunctions for Functions<'a> {
        type Env = AqueryEnvironment<'a>;

        fn get(&self, name: &str) -> Option<&dyn QueryFunction<AqueryEnvironment<'a>>> {
            if let Some(v) = self.extra_functions.get(name) {
                Some(v)
            } else {
                self.defaults.get(name)
            }
        }

        fn get_op(&self, op: BinaryOp) -> Option<&dyn QueryBinaryOp<AqueryEnvironment<'a>>> {
            if let Some(v) = self.extra_functions.get_op(op) {
                Some(v)
            } else {
                self.defaults.get_op(op)
            }
        }
    }

    Functions {
        defaults: DefaultQueryFunctionsModule::new(),
        extra_functions: AqueryFunctions(PhantomData),
    }
}

#[derive(Debug)]
pub(crate) struct AqueryFunctions<'a>(pub(crate) PhantomData<&'a ()>);

/// Aquery-specific
#[query_module(AqueryEnvironment<'a>)]
impl<'a> AqueryFunctions<'a> {
    /// Obtain the actions for all the outputs provided by the `DefaultInfo` for the targets passed
    /// as input. This includes both the `default_outputs` and `other_outputs`.
    ///
    /// This operation only makes sense on a target literal (it does nothing if passed something
    /// else).
    pub(crate) async fn all_outputs(
        &self,
        env: &AqueryEnvironment<'a>,
        targets: TargetSet<ActionQueryNode>,
    ) -> Result<QueryValue<ActionQueryNode>, QueryError> {
        let mut outputs = Vec::new();

        for target in &targets {
            if let Some(analysis) = target.analysis_opt() {
                analysis
                    .providers()?
                    .provider_collection()
                    .default_info()?
                    .for_each_output(&mut |output| outputs.push(output))?;
            }
        }

        let nodes = env.delegate.expand_artifacts(&outputs).await?;
        let nodes = nodes.into_iter().collect::<TargetSet<_>>().into();
        Ok(nodes)
    }

    /// Obtain all the actions declared within the analysis of a given target.
    ///
    /// This operation only makes sense on a target literal (it is a simple passthrough when passed
    /// an action).
    pub(crate) async fn all_actions(
        &self,
        env: &AqueryEnvironment<'a>,
        targets: TargetSet<ActionQueryNode>,
    ) -> Result<QueryValue<ActionQueryNode>, QueryError> {
        let mut res = TargetSet::new();
        let mut action_keys = Vec::new();

        for node in targets.into_iter() {
            match node.data() {
                ActionQueryNodeData::Action(..) => {
                    res.insert(node);
                }
                ActionQueryNodeData::Analysis(analysis) => {
                    for action in analysis.analysis_result().analysis_values()?.iter_actions() {
                        action_keys.push(action?.key().dupe());
                    }
                }
            }
        }

        let nodes = buck2_util::future::try_join_all(
            action_keys.iter().map(|key| env.delegate.get_node(key)),
        )
        .await?;
        res.extend(nodes);

        Ok(res.into())
    }
}
