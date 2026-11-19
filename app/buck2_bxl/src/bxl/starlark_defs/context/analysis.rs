/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_common::pagable::prepare_all;
use buck2_core::configuration::compatibility::IncompatiblePlatformReason;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use dice::DiceComputations;
use dupe::Dupe;
use either::Either;

use crate::bxl::starlark_defs::analysis_result::StarlarkAnalysisResult;
use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::context::ErrorPrinter;
use crate::bxl::starlark_defs::providers_expr::ProvidersExpr;

pub(crate) async fn analysis<'v>(
    dice: &mut DiceComputations<'_>,
    ctx: &BxlContext<'v>,
    expr: ProvidersExpr<ConfiguredProvidersLabel>,
    skip_incompatible: bool,
) -> buck2_error::Result<
    Either<Option<StarlarkAnalysisResult>, Vec<(ConfiguredProvidersLabel, StarlarkAnalysisResult)>>,
> {
    let analysis = dice
        .compute_join(expr.labels(), async |dice, label| {
            let maybe_result = dice
                .get_analysis_result(label.target())
                .await
                .ok()?
                .map(|r| r.dupe());
            buck2_error::Ok((label.clone(), maybe_result))
        })
        .await;
    // Resolving each label's providers reads paged-out fields; overlap those
    // reads on blocking threads instead of taking them in turn here.
    let analysis = prepare_all(analysis, |res| {
        let (label, maybe_result) = res?;
        let validated = maybe_result.try_map(|analysis_result| {
            buck2_error::Ok(StarlarkAnalysisResult::new(analysis_result.lookup(&label)?))
        })?;
        buck2_error::Ok((label, validated))
    })
    .await?
    .into_iter()
    .map(|res| {
        let (label, maybe_result) = res?;
        match maybe_result {
            MaybeCompatible::Incompatible(reason) => {
                if skip_incompatible {
                    ctx.print_to_error_stream(IncompatiblePlatformReason::skipping_message(
                        &reason,
                        label.target(),
                    ))?;
                    Ok(None)
                } else {
                    Err(reason.to_err())
                }
            }
            MaybeCompatible::Compatible(result) => Ok(Some((label, result))),
        }
    })
    .filter_map(|r| match r {
        Ok(r) => r.map(Ok),
        Err(e) => Some(Err(e)),
    })
    .collect::<buck2_error::Result<Vec<_>>>()?;

    match expr {
        ProvidersExpr::Literal(_) => {
            // If empty, then it is because we skipped an incompatible target
            let mut analysis = analysis.into_iter();
            let result = analysis.next().map(|(_, result)| result);
            assert!(
                analysis.next().is_none(),
                "a literal names at most one target"
            );
            Ok(Either::Left(result))
        }
        ProvidersExpr::Iterable(_) => Ok(Either::Right(analysis)),
    }
}
