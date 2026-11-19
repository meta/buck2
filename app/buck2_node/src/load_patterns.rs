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
use std::sync::Arc;

use buck2_common::file_ops::trait_::DiceFileOps;
use buck2_common::pattern::package_roots::collect_package_roots;
use buck2_common::pattern::resolve::ResolvedPattern;
use buck2_core::package::PackageLabel;
use buck2_core::package::PackageLabelWithModifiers;
use buck2_core::pattern::pattern::Modifiers;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern::ParsedPatternWithModifiers;
use buck2_core::pattern::pattern_type::PatternType;
use buck2_core::target::name::TargetName;
use buck2_events::dispatch::console_message;
use buck2_hash::BuckMutSet;
use dice::DiceComputations;
use dice::LinearRecomputeDiceComputations;
use dice_futures::owning_future::OwningFuture;
use dupe::Dupe;
use futures::FutureExt;
use futures::Stream;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use itertools::Itertools;

use crate::nodes::eval_result::EvaluationResult;
use crate::nodes::frontend::TargetGraphCalculation;
use crate::nodes::unconfigured::TargetNode;
use crate::nodes::unconfigured::TargetNodeRef;
use crate::super_package::SuperPackage;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum BuildErrors {
    #[error("Did not find package with name `{0}`.")]
    MissingPackage(PackageLabel),
}

struct Builder<'c, 'd> {
    ctx: LinearRecomputeDiceComputations<'c, 'd>,
    already_loading: BuckMutSet<PackageLabel>,
    load_package_futs:
        FuturesUnordered<BoxFuture<'c, (PackageLabel, buck2_error::Result<Arc<EvaluationResult>>)>>,
}

impl Builder<'_, '_> {
    pub fn new<'c, 'd>(ctx: LinearRecomputeDiceComputations<'c, 'd>) -> Builder<'c, 'd> {
        Builder {
            ctx,
            already_loading: BuckMutSet::default(),
            load_package_futs: FuturesUnordered::new(),
        }
    }

    fn load_package(&mut self, package: PackageLabel) {
        if !self.already_loading.insert(package.dupe()) {
            return;
        }

        // it's important that this is not async and the temporary spawn happens when the function is called as we don't immediately start polling these.
        // so DO NOT USE async move here
        self.load_package_futs.push(
            OwningFuture::new(self.ctx.get(), move |ctx| {
                ctx.get_interpreter_results(package)
                    .map(move |res| (package, res.map(|r| r.dupe())))
                    .boxed()
            })
            .boxed(),
        )
    }
}

async fn resolve_patterns_and_load_buildfiles<'c, 'd, T: PatternType>(
    ctx: LinearRecomputeDiceComputations<'c, 'd>,
    parsed_patterns: Vec<ParsedPattern<T>>,
) -> buck2_error::Result<(
    ResolvedPattern<T>,
    impl Stream<Item = (PackageLabel, buck2_error::Result<Arc<EvaluationResult>>)> + use<'c, 'd, T>,
)> {
    let mut spec = ResolvedPattern::<T>::new();
    let mut recursive_packages = Vec::new();

    let mut builder = Builder::new(ctx);
    for pattern in parsed_patterns {
        match pattern {
            ParsedPattern::Target(package, target_name, extra) => {
                spec.add_target(package.dupe(), target_name, extra, Modifiers::new(None));
                builder.load_package(package.dupe());
            }
            ParsedPattern::Package(package) => {
                spec.add_package(package.dupe(), Modifiers::new(None));
                builder.load_package(package.dupe());
            }
            ParsedPattern::Recursive(package) => {
                recursive_packages.push(package);
            }
        }
    }

    collect_package_roots(&DiceFileOps(ctx), recursive_packages, |package| {
        let package = package?;
        spec.add_package(package.dupe(), Modifiers::new(None));
        builder.load_package(package);
        buck2_error::Ok(())
    })
    .await?;

    Ok((spec, builder.load_package_futs))
}

async fn resolve_patterns_with_modifiers_and_load_buildfiles<'c, 'd, T: PatternType>(
    ctx: LinearRecomputeDiceComputations<'c, 'd>,
    parsed_patterns_with_modifiers: Vec<ParsedPatternWithModifiers<T>>,
) -> buck2_error::Result<(
    ResolvedPattern<T>,
    impl Stream<Item = (PackageLabel, buck2_error::Result<Arc<EvaluationResult>>)> + use<'c, 'd, T>,
)> {
    let mut spec = ResolvedPattern::<T>::new();

    let mut builder = Builder::new(ctx);
    for pattern_with_modifiers in parsed_patterns_with_modifiers {
        let ParsedPatternWithModifiers {
            parsed_pattern,
            modifiers,
        } = pattern_with_modifiers;

        match parsed_pattern {
            ParsedPattern::Target(package, target_name, extra) => {
                spec.add_target(package.dupe(), target_name, extra, modifiers);
                builder.load_package(package.dupe());
            }
            ParsedPattern::Package(package) => {
                spec.add_package(package.dupe(), modifiers);
                builder.load_package(package.dupe());
            }
            ParsedPattern::Recursive(cell_path) => {
                let mut roots = Vec::new();

                collect_package_roots(&DiceFileOps(ctx), vec![cell_path], |package| {
                    let package = package?;
                    roots.push(package);
                    buck2_error::Ok(())
                })
                .await?;

                for package in roots {
                    spec.add_package(package.dupe(), modifiers.dupe());
                    builder.load_package(package);
                }
            }
        }
    }

    Ok((spec, builder.load_package_futs))
}

pub struct LoadedPatterns<T: PatternType> {
    results: BTreeMap<PackageLabelWithModifiers, buck2_error::Result<PackageLoadedPatterns<T>>>,
}

pub struct PackageLoadedPatterns<T: PatternType> {
    targets: BTreeMap<(TargetName, T), TargetNode>,
    super_package: SuperPackage,
}

impl<T: PatternType> PackageLoadedPatterns<T> {
    pub fn iter(&self) -> impl Iterator<Item = (&(TargetName, T), TargetNodeRef<'_>)> {
        self.targets.iter().map(|(k, v)| (k, v.as_ref()))
    }

    pub fn keys(&self) -> impl Iterator<Item = &(TargetName, T)> {
        self.targets.keys()
    }

    pub fn values(&self) -> impl Iterator<Item = TargetNodeRef<'_>> {
        self.targets.values().map(|v| v.as_ref())
    }

    pub fn into_values(self) -> impl Iterator<Item = TargetNode> {
        self.targets.into_values()
    }

    pub fn super_package(&self) -> &SuperPackage {
        &self.super_package
    }
}

impl<T: PatternType> IntoIterator for PackageLoadedPatterns<T> {
    type Item = ((TargetName, T), TargetNode);
    type IntoIter = std::collections::btree_map::IntoIter<(TargetName, T), TargetNode>;

    fn into_iter(self) -> Self::IntoIter {
        self.targets.into_iter()
    }
}

impl<T: PatternType> LoadedPatterns<T> {
    pub fn iter(
        &self,
    ) -> impl Iterator<
        Item = (
            PackageLabelWithModifiers,
            &buck2_error::Result<PackageLoadedPatterns<T>>,
        ),
    > {
        self.results.iter().map(|(k, v)| (k.dupe(), v))
    }

    // Implementing IntoIterator requires explicitly specifying the iterator type, which seems higher cost than the value of doing it.
    #[allow(clippy::should_implement_trait)]
    pub fn into_iter(
        self,
    ) -> impl Iterator<
        Item = (
            PackageLabelWithModifiers,
            buck2_error::Result<PackageLoadedPatterns<T>>,
        ),
    > {
        self.results.into_iter()
    }

    pub fn iter_loaded_targets(
        &self,
    ) -> impl Iterator<Item = buck2_error::Result<TargetNodeRef<'_>>> {
        self.results
            .values()
            .map(|result| match result {
                Ok(pkg) => Ok(pkg.targets.values().map(|t| t.as_ref())),
                Err(e) => Err(e.dupe()),
            })
            .flatten_ok()
    }

    pub fn iter_loaded_targets_by_package(
        &self,
    ) -> impl Iterator<
        Item = (
            PackageLabelWithModifiers,
            buck2_error::Result<Vec<TargetNode>>,
        ),
    > + '_ {
        self.results.iter().map(|(package, result)| {
            let targets = result
                .as_ref()
                .map(|pkg| pkg.targets.values().map(|t| t.dupe()).collect::<Vec<_>>())
                .map_err(|e| e.dupe());
            (package.dupe(), targets)
        })
    }
}

/// Option to skip missing targets instead of failing.
/// This is not a good option to use long term, but we need it now to deal with our legacy setup.
#[derive(Clone, Dupe, Copy, Eq, PartialEq, Debug)]
pub enum MissingTargetBehavior {
    /// Skip missing targets (but error on missing packages or evaluation errors).
    /// When skipping, we emit a warning to the console.
    Warn,
    Fail,
}

impl MissingTargetBehavior {
    pub fn from_skip(skip: bool) -> MissingTargetBehavior {
        if skip {
            MissingTargetBehavior::Warn
        } else {
            MissingTargetBehavior::Fail
        }
    }
}

/// Finds all the requested targets in `spec` from a map of loaded targets in `load_result`.
fn apply_spec<T: PatternType>(
    spec: ResolvedPattern<T>,
    load_results: BTreeMap<PackageLabel, buck2_error::Result<Arc<EvaluationResult>>>,
    skip_missing_targets: MissingTargetBehavior,
) -> buck2_error::Result<LoadedPatterns<T>> {
    let mut all_targets: BTreeMap<_, buck2_error::Result<PackageLoadedPatterns<T>>> =
        BTreeMap::new();
    for (package_with_modifiers, pkg_spec) in spec.specs.into_iter() {
        let result = match load_results.get(&package_with_modifiers.package) {
            Some(r) => r,
            None => return Err(BuildErrors::MissingPackage(package_with_modifiers.package).into()),
        };
        match result {
            Ok(res) => {
                let (label_to_node, missing) = res.apply_spec(pkg_spec);
                if let Some(missing) = missing {
                    match skip_missing_targets {
                        MissingTargetBehavior::Fail => {
                            return Err(missing.into_first_error().into());
                        }
                        MissingTargetBehavior::Warn => {
                            console_message(missing.missing_targets_warning())
                        }
                    }
                };

                all_targets.insert(
                    package_with_modifiers,
                    Ok(PackageLoadedPatterns {
                        targets: label_to_node,
                        super_package: res.super_package().dupe(),
                    }),
                );
            }
            Err(e) => {
                all_targets.insert(package_with_modifiers, Err(e.dupe()));
            }
        }
    }

    Ok(LoadedPatterns {
        results: all_targets,
    })
}

pub async fn load_patterns<T: PatternType>(
    ctx: &mut DiceComputations<'_>,
    parsed_patterns: Vec<ParsedPattern<T>>,
    skip_missing_targets: MissingTargetBehavior,
) -> buck2_error::Result<LoadedPatterns<T>> {
    ctx.with_linear_recompute(|ctx| {
        async move {
            let (spec, mut load_package_futs) =
                resolve_patterns_and_load_buildfiles(ctx, parsed_patterns).await?;

            let mut results: BTreeMap<PackageLabel, buck2_error::Result<Arc<EvaluationResult>>> =
                BTreeMap::new();
            while let Some((pkg, load_res)) = load_package_futs.next().await {
                results.insert(pkg, load_res);
            }

            apply_spec(spec, results, skip_missing_targets)
        }
        .boxed()
    })
    .await
}

pub async fn load_patterns_with_modifiers<T: PatternType>(
    ctx: &mut DiceComputations<'_>,
    parsed_patterns: Vec<ParsedPatternWithModifiers<T>>,
    skip_missing_targets: MissingTargetBehavior,
) -> buck2_error::Result<LoadedPatterns<T>> {
    ctx.with_linear_recompute(|ctx| {
        async move {
            let (spec, mut load_package_futs) =
                resolve_patterns_with_modifiers_and_load_buildfiles(ctx, parsed_patterns).await?;

            let mut results: BTreeMap<PackageLabel, buck2_error::Result<Arc<EvaluationResult>>> =
                BTreeMap::new();
            while let Some((pkg, load_res)) = load_package_futs.next().await {
                results.insert(pkg, load_res);
            }

            apply_spec(spec, results, skip_missing_targets)
        }
        .boxed()
    })
    .await
}
