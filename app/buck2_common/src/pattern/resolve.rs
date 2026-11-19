/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::package::PackageLabel;
use buck2_core::package::PackageLabelWithModifiers;
use buck2_core::pattern::pattern::Modifiers;
use buck2_core::pattern::pattern::PackageSpec;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern::ParsedPatternWithModifiers;
use buck2_core::pattern::pattern::display_precise_pattern;
use buck2_core::pattern::pattern_type::ConfiguredProvidersPatternExtra;
use buck2_core::pattern::pattern_type::PatternType;
use buck2_core::target::name::TargetName;
use buck2_error::BuckErrorContext;
use buck2_hash::BuckMutInsertionMap;
use dice::DiceComputations;
use dupe::Dupe;
use futures::FutureExt;

use crate::file_ops::trait_::DiceFileOps;
use crate::file_ops::trait_::FileOps;
use crate::pattern::package_roots::find_package_roots;

/// Pattern where `foo/...` is expanded to matching packages.
/// Targets are not validated yet, and `:` is not yet expanded.
#[derive(Debug)]
pub struct ResolvedPattern<T: PatternType> {
    pub specs: BuckMutInsertionMap<PackageLabelWithModifiers, PackageSpec<T>>,
}

impl<T> ResolvedPattern<T>
where
    T: PatternType,
{
    pub fn new() -> Self {
        Self {
            specs: BuckMutInsertionMap::default(),
        }
    }

    pub fn add_package(&mut self, package: PackageLabel, modifiers: Modifiers) {
        self.specs.insert(
            PackageLabelWithModifiers { package, modifiers },
            PackageSpec::All(),
        );
    }

    pub fn add_target(
        &mut self,
        package: PackageLabel,
        target_name: TargetName,
        extra: T,
        modifiers: Modifiers,
    ) {
        let package_with_modifiers = PackageLabelWithModifiers { package, modifiers };

        if let Some(s) = self.specs.get_mut(&package_with_modifiers) {
            match s {
                PackageSpec::Targets(t) => t.push((target_name, extra)),
                PackageSpec::All() => {}
            }
        } else {
            self.specs.insert(
                package_with_modifiers,
                PackageSpec::Targets(vec![(target_name, extra)]),
            );
        }
    }
}

impl ResolvedPattern<ConfiguredProvidersPatternExtra> {
    pub fn convert_pattern<U: PatternType>(self) -> buck2_error::Result<ResolvedPattern<U>> {
        let mut specs = BuckMutInsertionMap::with_capacity(self.specs.len());
        for (package_with_modifiers, spec) in self.specs {
            let spec = match spec {
                PackageSpec::Targets(targets) => PackageSpec::Targets(
                    targets
                        .into_iter()
                        .map(|(target_name, extra)| {
                            let extra = U::from_configured_providers(extra.clone())
                                .with_buck_error_context(|| {
                                    format!(
                                        "Expecting {} pattern, got `{}`",
                                        U::NAME,
                                        display_precise_pattern(
                                            &package_with_modifiers.package,
                                            target_name.as_ref(),
                                            &extra,
                                        ),
                                    )
                                })?;
                            buck2_error::Ok((target_name, extra))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                PackageSpec::All() => PackageSpec::All(),
            };
            specs.insert(package_with_modifiers, spec);
        }
        Ok(ResolvedPattern { specs })
    }
}

pub struct ResolveTargetPatterns;

impl ResolveTargetPatterns {
    /// Resolves a list of [ParsedPattern] to a [ResolvedPattern].
    pub async fn resolve<P: PatternType>(
        ctx: &mut DiceComputations<'_>,
        patterns: &[ParsedPattern<P>],
    ) -> buck2_error::Result<ResolvedPattern<P>> {
        ctx.with_linear_recompute(|ctx| {
            async move { resolve_target_patterns_impl(patterns, &DiceFileOps(ctx)).await }.boxed()
        })
        .await
    }

    /// Resolves a list of [ParsedPatternWithModifiers] to a [ResolvedPattern].
    pub async fn resolve_with_modifiers<P: PatternType>(
        ctx: &mut DiceComputations<'_>,
        patterns: &[ParsedPatternWithModifiers<P>],
    ) -> buck2_error::Result<ResolvedPattern<P>> {
        ctx.with_linear_recompute(|ctx| {
            async move {
                resolve_target_patterns_with_modifiers_impl(patterns, &DiceFileOps(ctx)).await
            }
            .boxed()
        })
        .await
    }
}

async fn resolve_target_patterns_impl<P: PatternType>(
    patterns: &[ParsedPattern<P>],
    file_ops: &dyn FileOps,
) -> buck2_error::Result<ResolvedPattern<P>> {
    let mut resolved = ResolvedPattern::new();
    for pattern in patterns {
        match pattern {
            ParsedPattern::Target(package, target_name, extra) => {
                resolved.add_target(
                    package.dupe(),
                    target_name.clone(),
                    extra.clone(),
                    Modifiers::new(None),
                );
            }
            ParsedPattern::Package(package) => {
                resolved.add_package(package.dupe(), Modifiers::new(None));
            }
            ParsedPattern::Recursive(cell_path) => {
                let roots = find_package_roots(*cell_path, file_ops)
                    .await
                    .buck_error_context("Error resolving recursive target pattern.")?;
                for package in roots {
                    resolved.add_package(package, Modifiers::new(None));
                }
            }
        }
    }
    Ok(resolved)
}

async fn resolve_target_patterns_with_modifiers_impl<P: PatternType>(
    patterns: &[ParsedPatternWithModifiers<P>],
    file_ops: &dyn FileOps,
) -> buck2_error::Result<ResolvedPattern<P>> {
    let mut resolved = ResolvedPattern::new();

    for pattern in patterns {
        match &pattern.parsed_pattern {
            ParsedPattern::Target(package, target_name, extra) => {
                resolved.add_target(
                    package.dupe(),
                    target_name.clone(),
                    extra.clone(),
                    pattern.modifiers.clone(),
                );
            }
            ParsedPattern::Package(package) => {
                resolved.add_package(package.dupe(), pattern.modifiers.clone());
            }
            ParsedPattern::Recursive(cell_path) => {
                let roots = find_package_roots(*cell_path, file_ops)
                    .await
                    .buck_error_context("Error resolving recursive target pattern.")?;
                for package in roots {
                    resolved.add_package(package, pattern.modifiers.clone());
                }
            }
        }
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;
    use std::marker::PhantomData;
    use std::sync::Arc;

    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
    use buck2_core::package::PackageLabel;
    use buck2_core::package::PackageLabelWithModifiers;
    use buck2_core::pattern::pattern::Modifiers;
    use buck2_core::pattern::pattern::PackageSpec;
    use buck2_core::pattern::pattern::ParsedPattern;
    use buck2_core::pattern::pattern::ParsedPatternWithModifiers;
    use buck2_core::pattern::pattern_type::PatternType;
    use buck2_core::pattern::pattern_type::ProvidersPatternExtra;
    use buck2_core::pattern::pattern_type::TargetPatternExtra;
    use buck2_core::provider::label::NonDefaultProvidersName;
    use buck2_core::provider::label::ProviderName;
    use buck2_core::provider::label::ProvidersName;
    use buck2_core::target::name::TargetName;
    use dupe::Dupe;
    use test_case::test_case;

    use crate::file_ops::testing::TestFileOps;
    use crate::file_ops::trait_::FileOps;
    use crate::pattern::resolve::ResolvedPattern;
    use crate::pattern::resolve::resolve_target_patterns_impl;
    use crate::pattern::resolve::resolve_target_patterns_with_modifiers_impl;

    #[derive(Clone)]
    struct TestPatternResolver {
        resolver: CellResolver,
        file_ops: Arc<dyn FileOps>,
    }

    impl TestPatternResolver {
        fn new(cells: &[(&str, &str)], files: &[&str]) -> buck2_error::Result<Self> {
            let resolver = {
                let cells: Vec<_> = cells
                    .iter()
                    .map(|(name, path)| {
                        (
                            CellName::testing_new(name),
                            CellRootPathBuf::testing_new(path),
                        )
                    })
                    .collect();

                CellResolver::testing_with_names_and_paths(&cells)
            };

            let resolved_files = files
                .iter()
                .map(|p| {
                    (
                        resolver
                            .get_cell_path(&ProjectRelativePathBuf::unchecked_new((*p).to_owned())),
                        "".to_owned(),
                    )
                })
                .collect();

            let file_ops = Arc::new(TestFileOps::new_with_files(resolved_files));
            Ok(TestPatternResolver { resolver, file_ops })
        }

        async fn resolve<T>(&self, patterns: &[&str]) -> buck2_error::Result<ResolvedPattern<T>>
        where
            T: PatternType,
        {
            let patterns: Vec<_> = patterns
                .iter()
                .map(|p| {
                    ParsedPattern::<T>::parse_precise(
                        p,
                        CellName::testing_new("root"),
                        &self.resolver,
                        self.resolver.root_cell_cell_alias_resolver(),
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();

            resolve_target_patterns_impl(&patterns, &*self.file_ops).await
        }

        async fn resolve_with_modifiers<T>(
            &self,
            patterns: &[&str],
        ) -> buck2_error::Result<ResolvedPattern<T>>
        where
            T: PatternType,
        {
            let patterns: Vec<_> = patterns
                .iter()
                .map(|pattern_str| {
                    ParsedPatternWithModifiers::<T>::parse_precise(
                        pattern_str,
                        CellName::testing_new("root"),
                        &self.resolver,
                        self.resolver.root_cell_cell_alias_resolver(),
                    )
                    .unwrap()
                })
                .collect();

            resolve_target_patterns_with_modifiers_impl(&patterns, &*self.file_ops).await
        }
    }

    trait ResolvedTargetPatternTestExt<T: PatternType> {
        fn assert_eq(&self, expected: &[(PackageLabelWithModifiers, PackageSpec<T>)]);
    }

    impl<T> ResolvedTargetPatternTestExt<T> for ResolvedPattern<T>
    where
        T: PatternType,
    {
        fn assert_eq(&self, expected: &[(PackageLabelWithModifiers, PackageSpec<T>)]) {
            let expected: BTreeMap<_, _> = expected.iter().map(|(p, s)| (p.dupe(), s)).collect();

            let expected_keys: BTreeSet<_> = expected.keys().collect();
            let actual_keys: BTreeSet<_> = self.specs.keys().collect();

            let missing_keys: Vec<_> = expected_keys.difference(&actual_keys).collect();
            assert!(
                missing_keys.is_empty(),
                "Expected entries for keys {:?}. Had {:?}",
                missing_keys,
                self.specs
            );

            let extra_keys: Vec<_> = actual_keys.difference(&expected_keys).collect();
            assert!(extra_keys.is_empty(), "Got unexpected keys {extra_keys:?}");

            for (k, v) in expected {
                assert_eq!(v, self.specs.get(&k).unwrap());
            }
        }
    }

    #[tokio::test]
    async fn test_simple_specs_targets() -> buck2_error::Result<()> {
        let tester = TestPatternResolver::new(&[("root", ""), ("child", "child/cell")], &[])?;
        tester
            .resolve::<TargetPatternExtra>(&[])
            .await?
            .assert_eq(&[]);
        tester
            .resolve::<TargetPatternExtra>(&[
                "//some:target",
                "//some:other_target",
                "child//a/package:",
            ])
            .await?
            .assert_eq(&[
                (
                    PackageLabelWithModifiers {
                        package: PackageLabel::testing_parse("root//some"),
                        modifiers: Modifiers::new(None),
                    },
                    PackageSpec::Targets(vec![
                        (TargetName::testing_new("target"), TargetPatternExtra),
                        (TargetName::testing_new("other_target"), TargetPatternExtra),
                    ]),
                ),
                (
                    PackageLabelWithModifiers {
                        package: PackageLabel::testing_parse("child//a/package"),
                        modifiers: Modifiers::new(None),
                    },
                    PackageSpec::All(),
                ),
            ]);
        Ok(())
    }

    #[tokio::test]
    async fn test_simple_specs_providers() -> buck2_error::Result<()> {
        let tester = TestPatternResolver::new(&[("root", ""), ("child", "child/cell")], &[])?;
        tester
            .resolve::<ProvidersPatternExtra>(&[])
            .await?
            .assert_eq(&[]);
        tester
            .resolve::<ProvidersPatternExtra>(&[
                "//some:target",
                "//some:other_target[my-label]",
                "child//a/package:",
            ])
            .await?
            .assert_eq(&[
                (
                    PackageLabelWithModifiers {
                        package: PackageLabel::testing_parse("root//some"),
                        modifiers: Modifiers::new(None),
                    },
                    PackageSpec::Targets(vec![
                        (
                            TargetName::testing_new("target"),
                            ProvidersPatternExtra {
                                providers: ProvidersName::Default,
                            },
                        ),
                        (
                            TargetName::testing_new("other_target"),
                            ProvidersPatternExtra {
                                providers: ProvidersName::NonDefault(triomphe::Arc::new(
                                    NonDefaultProvidersName::Named(
                                        buck2_util::arc_str::ArcSlice::new([ProviderName::new(
                                            "my-label".to_owned(),
                                        )
                                        .unwrap()]),
                                    ),
                                )),
                            },
                        ),
                    ]),
                ),
                (
                    PackageLabelWithModifiers {
                        package: PackageLabel::testing_parse("child//a/package"),
                        modifiers: Modifiers::new(None),
                    },
                    PackageSpec::All(),
                ),
            ]);
        Ok(())
    }

    #[test_case(PhantomData::< TargetPatternExtra >; "parsing TargetPattern")]
    #[test_case(PhantomData::< ProvidersPatternExtra >; "parsing ProvidersPattern")]
    fn test_recursive_specs<T: PatternType>(_: PhantomData<T>) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tester = TestPatternResolver::new(
                &[("root", ""), ("child", "child/cell")],
                &[
                    ("BUCK"),
                    ("other/BUCK"),
                    ("other/a/bit/deeper/BUCK"),
                    ("other/a/bit/deeper/and/deeper/BUCK"),
                    ("some/thing/dir/a/BUCK"),
                    ("some/thing/dir/a/b/BUCK"),
                    ("some/thing/extra/BUCK"),
                    ("child/cell/BUCK"),
                    ("child/cell/foo/BUCK"),
                ],
            )
            .unwrap();
            tester.resolve::<T>(&[]).await.unwrap().assert_eq(&[]);
            tester
                .resolve::<T>(&[
                    "//other/...",
                    "//other:target_that_doesnt_matter",
                    "//some/...",
                    "//some/thing/extra/...",
                    // root of cell.
                    "child//...",
                ])
                .await
                .unwrap()
                .assert_eq(&[
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//other"),
                            modifiers: Modifiers::new(None),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//other/a/bit/deeper"),
                            modifiers: Modifiers::new(None),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse(
                                "root//other/a/bit/deeper/and/deeper",
                            ),
                            modifiers: Modifiers::new(None),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//some/thing/dir/a"),
                            modifiers: Modifiers::new(None),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//some/thing/dir/a/b"),
                            modifiers: Modifiers::new(None),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//some/thing/extra"),
                            modifiers: Modifiers::new(None),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("child//"),
                            modifiers: Modifiers::new(None),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("child//foo"),
                            modifiers: Modifiers::new(None),
                        },
                        PackageSpec::All(),
                    ),
                ]);
        })
    }

    #[tokio::test]
    async fn test_simple_specs_targets_with_modifiers() -> buck2_error::Result<()> {
        let tester = TestPatternResolver::new(&[("root", ""), ("child", "child/cell")], &[])?;

        tester
            .resolve_with_modifiers::<TargetPatternExtra>(&[])
            .await?
            .assert_eq(&[]);

        tester
            .resolve_with_modifiers::<TargetPatternExtra>(&[
                "//some:target?modifier1",
                "//some:other_target?modifier1+modifier2",
                "//some:third_target",
                "child//a/package:?package_modifier",
            ])
            .await?
            .assert_eq(&[
                (
                    PackageLabelWithModifiers {
                        package: PackageLabel::testing_parse("root//some"),
                        modifiers: Modifiers::new(Some(vec!["modifier1".to_owned()])),
                    },
                    PackageSpec::Targets(vec![(
                        TargetName::testing_new("target"),
                        TargetPatternExtra,
                    )]),
                ),
                (
                    PackageLabelWithModifiers {
                        package: PackageLabel::testing_parse("root//some"),
                        modifiers: Modifiers::new(Some(vec![
                            "modifier1".to_owned(),
                            "modifier2".to_owned(),
                        ])),
                    },
                    PackageSpec::Targets(vec![(
                        TargetName::testing_new("other_target"),
                        TargetPatternExtra,
                    )]),
                ),
                (
                    PackageLabelWithModifiers {
                        package: PackageLabel::testing_parse("root//some"),
                        modifiers: Modifiers::new(None),
                    },
                    PackageSpec::Targets(vec![(
                        TargetName::testing_new("third_target"),
                        TargetPatternExtra,
                    )]),
                ),
                (
                    PackageLabelWithModifiers {
                        package: PackageLabel::testing_parse("child//a/package"),
                        modifiers: Modifiers::new(Some(vec!["package_modifier".to_owned()])),
                    },
                    PackageSpec::All(),
                ),
            ]);
        Ok(())
    }

    #[tokio::test]
    async fn test_simple_specs_providers_with_modifiers() -> buck2_error::Result<()> {
        let tester = TestPatternResolver::new(&[("root", ""), ("child", "child/cell")], &[])?;

        tester
            .resolve_with_modifiers::<ProvidersPatternExtra>(&[
                "//some:other_target[my-label]?modifier",
            ])
            .await?
            .assert_eq(&[(
                PackageLabelWithModifiers {
                    package: PackageLabel::testing_parse("root//some"),
                    modifiers: Modifiers::new(Some(vec!["modifier".to_owned()])),
                },
                PackageSpec::Targets(vec![(
                    TargetName::testing_new("other_target"),
                    ProvidersPatternExtra {
                        providers: ProvidersName::NonDefault(triomphe::Arc::new(
                            NonDefaultProvidersName::Named(buck2_util::arc_str::ArcSlice::new([
                                ProviderName::new("my-label".to_owned()).unwrap(),
                            ])),
                        )),
                    },
                )]),
            )]);
        Ok(())
    }

    #[test_case(PhantomData::< TargetPatternExtra >; "parsing TargetPattern")]
    #[test_case(PhantomData::< ProvidersPatternExtra >; "parsing ProvidersPattern")]
    fn test_recursive_patterns_with_modifiers<T: PatternType>(_: PhantomData<T>) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tester = TestPatternResolver::new(
                &[("root", ""), ("child", "child/cell")],
                &[
                    ("BUCK"),
                    ("other/BUCK"),
                    ("other/a/bit/deeper/BUCK"),
                    ("other/a/bit/deeper/and/deeper/BUCK"),
                    ("some/thing/dir/a/BUCK"),
                    ("some/thing/dir/a/b/BUCK"),
                    ("some/thing/extra/BUCK"),
                    ("child/cell/BUCK"),
                    ("child/cell/foo/BUCK"),
                ],
            )
            .unwrap();

            tester
                .resolve_with_modifiers::<T>(&[
                    "//other/...?recursive_mod",
                    "//other:target_that_does_matter?modifier1",
                    "//some/...?modifier1",
                    "//some/thing/extra/...?modifier2",
                    "//some/thing/extra/...",
                    "child//...?cell_mod+another_mod",
                ])
                .await
                .unwrap()
                .assert_eq(&[
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//other"),
                            modifiers: Modifiers::new(Some(vec!["recursive_mod".to_owned()])),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//other/a/bit/deeper"),
                            modifiers: Modifiers::new(Some(vec!["recursive_mod".to_owned()])),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse(
                                "root//other/a/bit/deeper/and/deeper",
                            ),
                            modifiers: Modifiers::new(Some(vec!["recursive_mod".to_owned()])),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//other"),
                            modifiers: Modifiers::new(Some(vec!["modifier1".to_owned()])),
                        },
                        PackageSpec::Targets(vec![(
                            TargetName::testing_new("target_that_does_matter"),
                            T::default(),
                        )]),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//some/thing/dir/a"),
                            modifiers: Modifiers::new(Some(vec!["modifier1".to_owned()])),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//some/thing/dir/a/b"),
                            modifiers: Modifiers::new(Some(vec!["modifier1".to_owned()])),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//some/thing/extra"),
                            modifiers: Modifiers::new(Some(vec!["modifier1".to_owned()])),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//some/thing/extra"),
                            modifiers: Modifiers::new(Some(vec!["modifier2".to_owned()])),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("root//some/thing/extra"),
                            modifiers: Modifiers::new(None),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("child//"),
                            modifiers: Modifiers::new(Some(vec![
                                "cell_mod".to_owned(),
                                "another_mod".to_owned(),
                            ])),
                        },
                        PackageSpec::All(),
                    ),
                    (
                        PackageLabelWithModifiers {
                            package: PackageLabel::testing_parse("child//foo"),
                            modifiers: Modifiers::new(Some(vec![
                                "cell_mod".to_owned(),
                                "another_mod".to_owned(),
                            ])),
                        },
                        PackageSpec::All(),
                    ),
                ]);
        })
    }
}
