/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::pattern::parse_from_cli;
use buck2_common::pattern::resolve::ResolvedPattern;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::package::PackageLabelWithModifiers;
use buck2_core::pattern::pattern::ProvidersLabelWithModifiers;
use buck2_core::pattern::pattern::TargetLabelWithExtra;
use buck2_core::pattern::pattern_type::PatternType;
use buck2_core::pattern::pattern_type::ProvidersPatternExtra;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use dice::DiceComputations;
use dupe::Dupe;

async fn resolve_patterns_to_targets<T: PatternType>(
    ctx: &mut DiceComputations<'_>,
    resolved_pattern: ResolvedPattern<T>,
) -> buck2_error::Result<Vec<TargetLabelWithExtra<T>>> {
    let mut result_targets = Vec::new();
    for (package_with_modifiers, spec) in resolved_pattern.specs {
        let PackageLabelWithModifiers { package, modifiers } = package_with_modifiers;

        match spec {
            buck2_core::pattern::pattern::PackageSpec::Targets(targets) => result_targets.extend(
                targets
                    .into_iter()
                    .map(|(name, extra)| TargetLabelWithExtra {
                        target_label: TargetLabel::new(package.dupe(), name.as_ref()),
                        extra,
                        modifiers: modifiers.dupe(),
                    })
                    .collect::<Vec<_>>(),
            ),
            buck2_core::pattern::pattern::PackageSpec::All() => {
                // Note this code is not parallel. Careful if used in performance sensitive code.
                let interpreter_results = ctx.get_interpreter_results(package.dupe()).await?;
                result_targets.extend(interpreter_results.targets().keys().map(|target| {
                    TargetLabelWithExtra {
                        target_label: TargetLabel::new(package.dupe(), target),
                        extra: T::default(),
                        modifiers: modifiers.dupe(),
                    }
                }));
            }
        }
    }
    Ok(result_targets)
}

pub async fn parse_and_resolve_patterns_to_targets_from_cli_args<T: PatternType>(
    ctx: &mut DiceComputations<'_>,
    target_patterns: &[String],
    cwd: &ProjectRelativePath,
) -> buck2_error::Result<Vec<TargetLabelWithExtra<T>>> {
    let resolved_pattern =
        parse_from_cli::parse_and_resolve_patterns_from_cli_args::<T>(ctx, target_patterns, cwd)
            .await?;
    resolve_patterns_to_targets(ctx, resolved_pattern).await
}

pub async fn parse_and_resolve_patterns_with_modifiers_to_targets_from_cli_args<T: PatternType>(
    ctx: &mut DiceComputations<'_>,
    target_patterns: &[String],
    cwd: &ProjectRelativePath,
) -> buck2_error::Result<Vec<TargetLabelWithExtra<T>>> {
    let resolved_pattern =
        parse_from_cli::parse_and_resolve_patterns_with_modifiers_from_cli_args::<T>(
            ctx,
            target_patterns,
            cwd,
        )
        .await?;
    resolve_patterns_to_targets(ctx, resolved_pattern).await
}

pub async fn parse_and_resolve_provider_labels_from_cli_args(
    ctx: &mut DiceComputations<'_>,
    target_patterns: &[String],
    cwd: &ProjectRelativePath,
) -> buck2_error::Result<Vec<ProvidersLabel>> {
    let targets = parse_and_resolve_patterns_to_targets_from_cli_args::<ProvidersPatternExtra>(
        ctx,
        target_patterns,
        cwd,
    )
    .await?;
    Ok(targets
        .into_iter()
        .map(|t| t.into_providers_label())
        .collect::<Vec<_>>())
}

pub async fn parse_and_resolve_provider_labels_with_modifiers_from_cli_args(
    ctx: &mut DiceComputations<'_>,
    target_patterns: &[String],
    cwd: &ProjectRelativePath,
) -> buck2_error::Result<Vec<ProvidersLabelWithModifiers>> {
    let targets = parse_and_resolve_patterns_with_modifiers_to_targets_from_cli_args::<
        ProvidersPatternExtra,
    >(ctx, target_patterns, cwd)
    .await?;

    Ok(targets
        .into_iter()
        .map(|t| t.into_providers_label_with_modifiers())
        .collect::<Vec<_>>())
}
