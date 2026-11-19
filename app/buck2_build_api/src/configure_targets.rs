/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_core::configuration::compatibility::IncompatiblePlatformReason;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::package::PackageLabelWithModifiers;
use buck2_core::pattern::pattern::ModifiersError;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern::ParsedPatternWithModifiers;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_events::dispatch::console_message;
use buck2_node::load_patterns::MissingTargetBehavior;
use buck2_node::load_patterns::load_patterns;
use buck2_node::load_patterns::load_patterns_with_modifiers;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_node::target_calculation::ConfiguredTargetCalculation;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use dice::DiceComputations;
use dupe::Dupe;

// Returns a tuple of compatible targets, incompatible targets, and target-level errors.
// NOTE: This function returns Result<(..., Vec<Error>)> to support keep-going:
// - When keep_going = false: Returns Err(e) immediately on first error
// - When keep_going = true: Returns Ok((..., errors)) with all errors collected in Vec
fn split_compatible_incompatible(
    targets: impl IntoIterator<Item = ResultMaybeCompatible<ConfiguredTargetNode>>,
    keep_going: bool,
) -> buck2_error::Result<(
    TargetSet<ConfiguredTargetNode>,
    Vec<Arc<IncompatiblePlatformReason>>,
    Vec<buck2_error::Error>,
)> {
    let mut target_set = TargetSet::new();
    let mut incompatible_targets = Vec::new();
    let mut errors = Vec::new();

    for res in targets {
        match res {
            ResultMaybeCompatible::Incompatible(reason) => {
                incompatible_targets.push(reason);
            }
            ResultMaybeCompatible::Compatible(target) => {
                target_set.insert(target);
            }
            ResultMaybeCompatible::Err(e) => {
                if keep_going {
                    errors.push(e);
                } else {
                    return Err(e);
                }
            }
        }
    }
    Ok((target_set, incompatible_targets, errors))
}

// Errors that occurr during pattern loading or target configuration.
// Package info is available for package-level errors, but may be None for target-level configuration errors.
pub struct ErrorWithPackageLabel {
    pub package: Option<PackageLabelWithModifiers>,
    pub error: buck2_error::Error,
}

pub async fn get_maybe_compatible_targets<T>(
    ctx: &mut DiceComputations<'_>,
    loaded_targets: T,
    global_cfg_options: &GlobalCfgOptions,
    keep_going: bool,
) -> buck2_error::Result<(
    impl Iterator<Item = ResultMaybeCompatible<ConfiguredTargetNode>> + use<T>,
    Vec<ErrorWithPackageLabel>,
)>
where
    T: IntoIterator<
        Item = (
            PackageLabelWithModifiers,
            buck2_error::Result<Vec<TargetNode>>,
        ),
    >,
{
    let mut by_package_fns: Vec<_> = Vec::new();
    let mut package_errors = Vec::new();

    for (package_with_modifiers, result) in loaded_targets {
        match result {
            Ok(targets) => {
                let local_cfg_options = match package_with_modifiers.modifiers.as_slice() {
                    Some(modifiers) => {
                        if !global_cfg_options.cli_modifiers.is_empty() {
                            let error = buck2_error::Error::from(
                                ModifiersError::PatternModifiersWithGlobalModifiers,
                            );
                            if keep_going {
                                package_errors.push(ErrorWithPackageLabel {
                                    package: Some(package_with_modifiers),
                                    error,
                                });
                                continue;
                            } else {
                                return Err(error);
                            }
                        }

                        GlobalCfgOptions {
                            target_platform: global_cfg_options.target_platform.dupe(),
                            cli_modifiers: modifiers.to_vec().into(),
                        }
                    }
                    None => global_cfg_options.dupe(),
                };

                let target_fns = targets.into_iter().map(|target| {
                    let duped_cfg_options = local_cfg_options.dupe();
                    DiceComputations::declare_closure(async move |ctx| {
                        let target = ctx
                            .get_configured_target(target.label(), &duped_cfg_options)
                            .await?;
                        ctx.get_configured_target_node(&target)
                            .await
                            .map(Dupe::dupe)
                    })
                });

                by_package_fns.extend(target_fns);
            }
            Err(e) => {
                // TODO(@wendyy) - log the error
                if keep_going {
                    package_errors.push(ErrorWithPackageLabel {
                        package: Some(package_with_modifiers),
                        error: e,
                    });
                } else {
                    return Err(e);
                }
            }
        }
    }

    Ok((
        buck2_util::future::join_all(ctx.compute_many(by_package_fns))
            .await
            .into_iter(),
        package_errors,
    ))
}

pub struct ConfiguredTargetsWithErrors {
    pub compatible_targets: TargetSet<ConfiguredTargetNode>,
    pub incompatible_targets: Vec<Arc<IncompatiblePlatformReason>>,
    pub errors: Vec<ErrorWithPackageLabel>,
}

// Converts target nodes to a set of compatible configured target nodes.
pub async fn get_compatible_targets(
    ctx: &mut DiceComputations<'_>,
    loaded_targets: impl IntoIterator<
        Item = (
            PackageLabelWithModifiers,
            buck2_error::Result<Vec<TargetNode>>,
        ),
    >,
    global_cfg_options: &GlobalCfgOptions,
    keep_going: bool,
) -> buck2_error::Result<ConfiguredTargetsWithErrors> {
    let (maybe_compatible_targets, package_errors) =
        get_maybe_compatible_targets(ctx, loaded_targets, global_cfg_options, keep_going).await?;

    let (compatible_targets, incompatible_targets, target_errors) =
        split_compatible_incompatible(maybe_compatible_targets, keep_going)?;

    if !incompatible_targets.is_empty() {
        console_message(IncompatiblePlatformReason::skipping_message_for_multiple(
            incompatible_targets.iter().map(|r| &r.target),
        ));
    }

    let mut errors = package_errors;
    errors.extend(
        target_errors
            .into_iter()
            .map(|error| ErrorWithPackageLabel {
                package: None, // Target-level errors don't have package context
                error,
            }),
    );

    Ok(ConfiguredTargetsWithErrors {
        compatible_targets,
        incompatible_targets,
        errors,
    })
}

pub async fn load_compatible_patterns(
    ctx: &mut DiceComputations<'_>,
    parsed_patterns: Vec<ParsedPattern<TargetPatternExtra>>,
    global_cfg_options: &GlobalCfgOptions,
    skip_missing_targets: MissingTargetBehavior,
    keep_going: bool,
) -> buck2_error::Result<ConfiguredTargetsWithErrors> {
    let loaded_patterns = load_patterns(ctx, parsed_patterns, skip_missing_targets).await?;
    get_compatible_targets(
        ctx,
        loaded_patterns.iter_loaded_targets_by_package(),
        global_cfg_options,
        keep_going,
    )
    .await
}

pub async fn load_compatible_patterns_with_modifiers(
    ctx: &mut DiceComputations<'_>,
    parsed_patterns_with_modifiers: Vec<ParsedPatternWithModifiers<TargetPatternExtra>>,
    global_cfg_options: &GlobalCfgOptions,
    skip_missing_targets: MissingTargetBehavior,
    keep_going: bool,
) -> buck2_error::Result<ConfiguredTargetsWithErrors> {
    let loaded_patterns_with_modifiers =
        load_patterns_with_modifiers(ctx, parsed_patterns_with_modifiers, skip_missing_targets)
            .await?;
    get_compatible_targets(
        ctx,
        loaded_patterns_with_modifiers.iter_loaded_targets_by_package(),
        global_cfg_options,
        keep_going,
    )
    .await
}
