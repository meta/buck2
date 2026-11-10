/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_error::starlark_error::from_starlark_with_options;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationResult;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationValue;
use starlark::typing::HasTyVTable;
use starlark::values::Heap;
use starlark::values::Value;

use super::targetset::NodeLike;
use crate::bxl::starlark_defs::file_set::StarlarkFileSet;
use crate::bxl::starlark_defs::targetset::StarlarkTargetSet;

pub(crate) fn parse_query_evaluation_result<'v, T: NodeLike>(
    result: QueryEvaluationResult<T>,
    heap: Heap<'v>,
) -> buck2_error::Result<Value<'v>>
where
    StarlarkTargetSet<T>: HasTyVTable,
{
    Ok(match result {
        QueryEvaluationResult::Single(result) => match result {
            QueryEvaluationValue::TargetSet(targets) => heap
                .alloc_representation(StarlarkTargetSet::from(targets))
                .to_value(),
            QueryEvaluationValue::FileSet(files) => heap
                .alloc_representation(StarlarkFileSet::from(files))
                .to_value(),
        },
        QueryEvaluationResult::Multiple(multi) => heap.alloc_dict(
            multi
                .0
                .into_iter()
                .map(|(q, res)| {
                    Ok((
                        heap.alloc(q).get_hashed().map_err(|e| {
                            from_starlark_with_options(
                                e,
                                buck2_error::starlark_error::NativeErrorHandling::Unknown,
                                false,
                            )
                        })?,
                        match res? {
                            QueryEvaluationValue::TargetSet(targets) => heap
                                .alloc_representation(StarlarkTargetSet::from(targets))
                                .to_value(),
                            QueryEvaluationValue::FileSet(files) => heap
                                .alloc_representation(StarlarkFileSet::from(files))
                                .to_value(),
                        },
                    ))
                })
                .collect::<buck2_error::Result<_>>()?,
        ),
    })
}
