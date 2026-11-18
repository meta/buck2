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
use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use allocative::Allocative;
use buck2_common::pattern::resolve::ResolvedPattern;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::pattern::PackageSpec;
use buck2_core::pattern::pattern_type::PatternType;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_core::target::name::TargetNameRef;
use buck2_events::dispatch::span;
use buck2_query::query::syntax::simple::eval::label_indexed::LabelIndexed;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_util::late_binding::LateBinding;
use buck2_util::self_ref::RefData;
use buck2_util::self_ref::SelfRef;
use dice::DiceComputations;
use dupe::Dupe;
use either::Either;
use itertools::Itertools;

use crate::nodes::configured::ConfiguredTargetNode;
use crate::nodes::configured::ConfiguredTargetNodeRef;
use crate::nodes::configured_node_visit_all_deps::configured_node_visit_all_deps;
use crate::rule_type::RuleType;

pub static UNIVERSE_FROM_LITERALS: LateBinding<
    for<'c> fn(
        &'c mut DiceComputations<'_>,
        &'c ProjectRelativePath,
        &'c [String],
        GlobalCfgOptions,
    )
        -> Pin<Box<dyn Future<Output = buck2_error::Result<CqueryUniverse>> + Send + 'c>>,
> = LateBinding::new("UNIVERSE_FROM_LITERALS");

#[derive(Debug)]
struct CqueryUniverseInner<'a> {
    targets: BTreeMap<
        PackageLabel,
        BTreeMap<&'a TargetNameRef, BTreeSet<LabelIndexed<ConfiguredTargetNodeRef<'a>>>>,
    >,
}

struct CqueryUniverseInnerType;

impl RefData for CqueryUniverseInnerType {
    type Data<'a> = CqueryUniverseInner<'a>;
}

/// Subset of targets `cquery` command works with.
///
/// Targets are resolved in the universe, and file owners are also resolved in the universe.
#[derive(Allocative, Debug)]
pub struct CqueryUniverse {
    data: SelfRef<CqueryUniverseInnerType>,
}

// SAFETY: `_assert_send_sync` requires the only field to be `Send + Sync`. Spelled out
// so that auto-trait proofs for query futures stop here; see `dice::DiceComputations`.
unsafe impl Send for CqueryUniverse {}
unsafe impl Sync for CqueryUniverse {}

fn _assert_send_sync(CqueryUniverse { data }: &CqueryUniverse) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(data);
}

impl<'a> CqueryUniverseInner<'a> {
    pub fn new(
        targets: BTreeMap<
            PackageLabel,
            BTreeMap<&'a TargetNameRef, BTreeSet<LabelIndexed<ConfiguredTargetNodeRef<'a>>>>,
        >,
    ) -> Self {
        CqueryUniverseInner { targets }
    }

    fn build_inner(
        universe: &'a TargetSet<ConfiguredTargetNode>,
    ) -> buck2_error::Result<CqueryUniverseInner<'a>> {
        let mut targets: BTreeMap<
            PackageLabel,
            BTreeMap<&TargetNameRef, BTreeSet<LabelIndexed<ConfiguredTargetNodeRef>>>,
        > = BTreeMap::new();

        configured_node_visit_all_deps(universe.iter().map(|t| t.as_ref()), |target| {
            let label = target.label();
            let package_targets: &mut _ = targets.entry(label.pkg().dupe()).or_default();

            let nodes: &mut _ = match package_targets.get_mut(label.name()) {
                Some(v) => v,
                None => package_targets.entry(label.name()).or_default(),
            };

            let inserted = nodes.insert(LabelIndexed(target));
            assert!(inserted, "Visited targets must be unique");
        });

        Ok(CqueryUniverseInner::new(targets))
    }
}

impl CqueryUniverse {
    pub fn len(&self) -> usize {
        self.data
            .data()
            .targets
            .values()
            .map(|e| e.values().len())
            .sum()
    }

    pub fn iter(&self) -> impl Iterator<Item = ConfiguredTargetNodeRef<'_>> {
        self.data
            .data()
            .targets
            .values()
            .flat_map(|map| map.values().flat_map(|set| set.iter().map(|node| node.0)))
    }

    pub fn build(
        universe: &TargetSet<ConfiguredTargetNode>,
    ) -> buck2_error::Result<CqueryUniverse> {
        span(buck2_data::CqueryUniverseBuildStart {}, || {
            let r = SelfRef::try_new(universe.clone(), |universe| {
                CqueryUniverseInner::build_inner(universe)
            })
            .map(|data| CqueryUniverse { data });
            (r, buck2_data::CqueryUniverseBuildEnd {})
        })
    }

    pub fn get(
        &self,
        resolved_pattern: &ResolvedPattern<TargetPatternExtra>,
    ) -> TargetSet<ConfiguredTargetNode> {
        let mut targets = TargetSet::new();
        for (package_with_modifiers, spec) in &resolved_pattern.specs {
            targets.extend(
                self.get_from_package(package_with_modifiers.package, spec)
                    .map(|(node, TargetPatternExtra)| node.to_owned()),
            );
        }
        targets
    }

    /// Used for BXL target universe lookup. BXL queries take in target expressions and convert them into `TargetSet<TargetNode>`.
    /// We can use each `TargetNode`'s package and target name to lookup the configured nodes in the target universe.
    pub fn get_from_targets(
        &self,
        targets: impl IntoIterator<Item = TargetLabel>,
    ) -> TargetSet<ConfiguredTargetNode> {
        let mut configured_nodes = TargetSet::new();
        for label in targets {
            let package = label.pkg();
            let name = label.name();
            let results = self
                .data
                .data()
                .targets
                .get(&package)
                .into_iter()
                .flat_map(move |package_universe| package_universe.get(name).into_iter().flatten())
                .map(|node| node.0.to_owned());

            configured_nodes.extend(results);
        }
        configured_nodes
    }

    pub fn get_target_label(&self, label: &TargetLabel) -> Vec<ConfiguredTargetLabel> {
        self.get_from_package(
            label.pkg(),
            &PackageSpec::Targets(vec![(label.name().to_owned(), TargetPatternExtra)]),
        )
        .map(|(node, _extra)| node.label().dupe())
        .collect()
    }

    pub fn contains(&self, label: &ConfiguredTargetLabel) -> bool {
        self.get_target_label(label.unconfigured())
            .iter()
            .any(|t| t == label)
    }

    pub fn get_provider_labels<P: PatternType>(
        &self,
        resolved_pattern: &ResolvedPattern<P>,
    ) -> Vec<ConfiguredProvidersLabel> {
        let mut targets = Vec::new();
        for (package_with_modifiers, spec) in &resolved_pattern.specs {
            targets.extend(
                self.get_from_package(package_with_modifiers.package, spec)
                    .filter_map(|(node, extra)| match node.rule_type() {
                        RuleType::Forward => None,
                        RuleType::Starlark(..) => Some(ConfiguredProvidersLabel::new(
                            node.label().dupe(),
                            extra.into_providers(),
                        )),
                    }),
            );
        }
        targets
    }

    fn get_from_package<'a, P: PatternType>(
        &'a self,
        package: PackageLabel,
        spec: &'a PackageSpec<P>,
    ) -> impl Iterator<Item = (ConfiguredTargetNodeRef<'a>, P)> + 'a {
        self.data
            .data()
            .targets
            .get(&package)
            .into_iter()
            .flat_map(move |package_universe| match spec {
                PackageSpec::Targets(names) => {
                    Either::Left(names.iter().flat_map(|(name, extra)| {
                        package_universe
                            .get(name.as_ref())
                            .into_iter()
                            .flat_map(|nodes| {
                                nodes.iter().filter_map(|node| {
                                    if extra.matches_cfg(node.0.label().cfg()) {
                                        Some((node.0, extra.clone()))
                                    } else {
                                        None
                                    }
                                })
                            })
                    }))
                }
                PackageSpec::All() => Either::Right(
                    package_universe
                        .values()
                        .flatten()
                        .map(|node| (node.0, P::default())),
                ),
            })
    }

    pub fn owners(&self, path: &CellPath) -> buck2_error::Result<Vec<ConfiguredTargetNode>> {
        let mut nodes = Vec::new();

        // We lookup in all ancestors because we still have package boundary violations.
        // But we ignore symlinks: path may be a symlink target
        // for a target living in another directory.
        // This is another reason to not support symlinks in buck.
        for package in path.ancestors() {
            // Here we allocate package for possibly non-existent package,
            // violating `Package` assumptions.
            // This does not leave this function, so we are probably fine.
            // We do it because the map is by `Package`,
            // and `BTreeMap` does not allow lookup by equivalent key.
            let package = PackageLabel::from_cell_path(package)?;
            let package_data = match self.data.data().targets.get(&package) {
                None => continue,
                Some(package_data) => package_data,
            };
            for node in package_data.values().flatten() {
                if node.0.body()?.inputs().contains(path) {
                    nodes.push(node.0.to_owned());
                }
            }
        }
        Ok(nodes)
    }
}

#[cfg(test)]
mod tests {
    use buck2_common::pattern::resolve::ResolvedPattern;
    use buck2_core::configuration::bound_label::BoundConfigurationLabel;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::configuration::hash::ConfigurationHash;
    use buck2_core::execution_types::execution::ExecutionPlatformResolution;
    use buck2_core::package::PackageLabel;
    use buck2_core::package::PackageLabelWithModifiers;
    use buck2_core::pattern::pattern::Modifiers;
    use buck2_core::pattern::pattern::PackageSpec;
    use buck2_core::pattern::pattern_type::ConfigurationPredicate;
    use buck2_core::pattern::pattern_type::ConfiguredProvidersPatternExtra;
    use buck2_core::provider::label::ConfiguredProvidersLabel;
    use buck2_core::provider::label::NonDefaultProvidersName;
    use buck2_core::provider::label::ProviderName;
    use buck2_core::provider::label::ProvidersName;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_core::target::name::TargetName;
    use buck2_hash::BuckMutInsertionMap;
    use buck2_query::query::syntax::simple::eval::set::TargetSet;
    use dupe::Dupe;

    use crate::configured_universe::CqueryUniverse;
    use crate::nodes::configured::ConfiguredTargetNode;

    #[tokio::test]
    async fn test_get_from_package_by_configured_provider_pattern() {
        fn providers_name() -> ProvidersName {
            ProvidersName::NonDefault(triomphe::Arc::new(NonDefaultProvidersName::Named(
                buck2_util::arc_str::ArcSlice::new([ProviderName::new("P".to_owned()).unwrap()]),
            )))
        }

        fn resolved_pattern(
            cfg: ConfigurationPredicate,
        ) -> ResolvedPattern<ConfiguredProvidersPatternExtra> {
            ResolvedPattern {
                specs: BuckMutInsertionMap::from_iter([(
                    PackageLabelWithModifiers {
                        package: PackageLabel::testing_parse("foo//bar"),
                        modifiers: Modifiers::new(None),
                    },
                    PackageSpec::Targets(Vec::from_iter([(
                        TargetName::testing_new("baz"),
                        ConfiguredProvidersPatternExtra {
                            providers: providers_name(),
                            cfg,
                            exec_cfg: ConfigurationPredicate::Any,
                        },
                    )])),
                )]),
            }
        }

        let target_label =
            ConfiguredTargetLabel::testing_parse("foo//bar:baz", ConfigurationData::testing_new());
        let universe =
            CqueryUniverse::build(&TargetSet::from_iter([ConfiguredTargetNode::testing_new(
                target_label.dupe(),
                "idris_library",
                ExecutionPlatformResolution::new_for_testing(None, Vec::new()),
                vec![],
                None,
            )]))
            .unwrap();
        let provider_label = ConfiguredProvidersLabel::new(target_label, providers_name());

        // Any configuration.
        assert_eq!(
            Vec::from_iter([provider_label.dupe()]),
            universe.get_provider_labels(&resolved_pattern(ConfigurationPredicate::Any))
        );
        // Configuration label.
        assert_eq!(
            Vec::from_iter([provider_label.dupe()]),
            universe.get_provider_labels(&resolved_pattern(ConfigurationPredicate::Bound(
                BoundConfigurationLabel::new(
                    ConfigurationData::testing_new().label().unwrap().to_owned()
                )
                .unwrap(),
                None,
            )))
        );
        // Configuration label with hash.
        assert_eq!(
            Vec::from_iter([provider_label]),
            universe.get_provider_labels(&resolved_pattern(ConfigurationPredicate::Bound(
                BoundConfigurationLabel::new(
                    ConfigurationData::testing_new().label().unwrap().to_owned()
                )
                .unwrap(),
                Some(ConfigurationData::testing_new().output_hash().clone()),
            )))
        );
        // Configuration label with wrong hash.
        assert_eq!(
            Vec::<ConfiguredProvidersLabel>::new(),
            universe.get_provider_labels(&resolved_pattern(ConfigurationPredicate::Bound(
                BoundConfigurationLabel::new(
                    ConfigurationData::testing_new().label().unwrap().to_owned()
                )
                .unwrap(),
                Some(ConfigurationHash::new(17)),
            )))
        );
    }
}
