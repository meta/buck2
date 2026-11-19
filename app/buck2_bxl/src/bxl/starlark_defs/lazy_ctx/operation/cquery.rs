/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use async_recursion::async_recursion;
use buck2_build_api::query::oneshot::QUERY_FRONTEND;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationResult;
use dice::DiceComputations;
use starlark::values::Heap;
use starlark::values::Value;

use crate::bxl::starlark_defs::context::BxlContextCoreData;
use crate::bxl::starlark_defs::query_util::parse_query_evaluation_result;

#[derive(Debug, Allocative)]
pub(crate) struct LazyCqueryOperation {
    global_cfg_options: GlobalCfgOptions,
    args: LazyCqueryArg,
}

#[derive(Debug, Allocative)]
pub(crate) enum LazyCqueryArg {
    Eval {
        query: String,
        query_args: Vec<String>,
        target_universe: Option<Vec<String>>,
    },
}

impl LazyCqueryOperation {
    pub(crate) fn new_eval(
        global_cfg_options: GlobalCfgOptions,
        query: String,
        query_args: Vec<String>,
        target_universe: Option<Vec<String>>,
    ) -> Self {
        Self {
            global_cfg_options,
            args: LazyCqueryArg::Eval {
                query,
                query_args,
                target_universe,
            },
        }
    }
}

pub(crate) enum LazyCqueryResult {
    Eval(QueryEvaluationResult<ConfiguredTargetNode>),
}

impl LazyCqueryOperation {
    #[async_recursion]
    pub(crate) async fn resolve(
        &self,
        dice: &mut DiceComputations<'_>,
        core_data: &BxlContextCoreData,
    ) -> buck2_error::Result<LazyCqueryResult> {
        match &self.args {
            LazyCqueryArg::Eval {
                query,
                query_args,
                target_universe,
            } => {
                let res = QUERY_FRONTEND
                    .get()?
                    .eval_cquery(
                        dice,
                        &core_data.working_dir()?,
                        query,
                        query_args,
                        self.global_cfg_options.clone(),
                        target_universe.as_ref().map(|items| &items[..]),
                        false,
                    )
                    .await?
                    .0;
                Ok(LazyCqueryResult::Eval(res))
            }
        }
    }
}

impl LazyCqueryResult {
    pub(crate) fn into_value<'v>(self, heap: Heap<'v>) -> buck2_error::Result<Value<'v>> {
        match self {
            LazyCqueryResult::Eval(result) => parse_query_evaluation_result(result, heap),
        }
    }
}
