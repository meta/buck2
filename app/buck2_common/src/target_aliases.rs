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
use async_trait::async_trait;
use buck2_core::target_aliases::TargetAliasResolver;
use buck2_hash::BuckMutInsertionSet;
use derive_more::Display;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use itertools::Itertools;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::dice::cells::HasCellResolver;
use crate::legacy_configs::configs::LegacyBuckConfig;
use crate::legacy_configs::dice::HasLegacyConfigs;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
enum AliasResolutionError {
    #[error("No [alias] section in buckconfig")]
    MissingAliasSection,
    #[error("[alias] section does not contain the requested alias")]
    NotAnAlias,
    #[error("[alias] section produced a dangling chain: `{}`", .0.iter().join(" -> "))]
    AliasChainBroken(Vec<String>),
    #[error("cycle detected in alias resolution [{} -> {1}]", .0.iter().join(" -> "))]
    AliasCycle(Vec<String>, String),
}

#[derive(Debug, Dupe, Clone, Allocative, Pagable)]
pub struct BuckConfigTargetAliasResolver {
    config: LegacyBuckConfig,
}

impl PartialEq for BuckConfigTargetAliasResolver {
    fn eq(&self, other: &BuckConfigTargetAliasResolver) -> bool {
        // `TargetAliasResolver` only uses `alias` section of buckconfig,
        // comparing only this section is enough.
        // Please update this code if `TargetAliasResolver` uses other buckconfigs.
        let self_aliases = self.config.get_section("alias");
        let other_aliases = other.config.get_section("alias");
        match (self_aliases, other_aliases) {
            (Some(self_aliases), Some(other_aliases)) => self_aliases.compare(other_aliases),
            (None, None) => true,
            (None, Some(_)) | (Some(_), None) => false,
        }
    }
}

impl TargetAliasResolver for BuckConfigTargetAliasResolver {
    fn get<'a>(&'a self, name: &str) -> buck2_error::Result<Option<&'a str>> {
        match self.resolve_alias(name) {
            Ok(a) => Ok(Some(a)),
            Err(AliasResolutionError::MissingAliasSection | AliasResolutionError::NotAnAlias) => {
                Ok(None)
            }
            Err(
                e @ AliasResolutionError::AliasChainBroken(..)
                | e @ AliasResolutionError::AliasCycle(..),
            ) => {
                Err(buck2_error::Error::from(e).context(format!("Error resolving alias `{name}`")))
            }
        }
    }
}

impl BuckConfigTargetAliasResolver {
    fn new(config: LegacyBuckConfig) -> Self {
        Self { config }
    }

    /// Resolves an alias in the `[alias]` section. Aliases can refer to other aliases. Any
    /// string containing ":" is considered to be the end of the alias resolution.
    fn resolve_alias<'a>(&'a self, alias: &str) -> Result<&'a str, AliasResolutionError> {
        if alias.contains(':') {
            return Err(AliasResolutionError::NotAnAlias);
        }

        let mut alias = alias;

        let section = self.config.get_section("alias");
        let mut stack = BuckMutInsertionSet::<&str>::default();
        loop {
            if stack.contains(alias) {
                return Err(AliasResolutionError::AliasCycle(
                    stack.into_iter().map(|s| s.to_owned()).collect(),
                    alias.to_owned(),
                ));
            }

            let new_alias = match &section {
                Some(section) => match section.get(alias) {
                    Some(v) => {
                        stack.insert(alias);
                        v.as_str()
                    }
                    None => {
                        if stack.is_empty() {
                            return Err(AliasResolutionError::NotAnAlias);
                        } else {
                            let chain = stack
                                .into_iter()
                                .chain(std::iter::once(alias))
                                .map(|e| e.to_owned())
                                .collect();
                            return Err(AliasResolutionError::AliasChainBroken(chain));
                        }
                    }
                },
                None => return Err(AliasResolutionError::MissingAliasSection),
            };

            if new_alias.contains(':') {
                return Ok(new_alias);
            }
            alias = new_alias;
        }
    }
}

#[async_trait]
pub trait HasTargetAliasResolver<'d> {
    async fn target_alias_resolver(
        &mut self,
    ) -> buck2_error::Result<&'d BuckConfigTargetAliasResolver>;
}

#[derive(Debug, Display, Hash, PartialEq, Eq, Clone, Allocative, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct TargetAliasResolverKey();

#[async_trait]
impl Key for TargetAliasResolverKey {
    type Value = buck2_error::Result<BuckConfigTargetAliasResolver>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> buck2_error::Result<BuckConfigTargetAliasResolver> {
        let root_cell = ctx.get_cell_resolver().await?.root_cell();
        let legacy_configs = ctx.get_legacy_config_for_cell(root_cell).await?;
        Ok(BuckConfigTargetAliasResolver::new(legacy_configs.dupe()))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

#[async_trait]
impl<'d> HasTargetAliasResolver<'d> for DiceComputations<'d> {
    async fn target_alias_resolver(
        &mut self,
    ) -> buck2_error::Result<&'d BuckConfigTargetAliasResolver> {
        self.compute(&TargetAliasResolverKey())
            .await?
            .as_ref()
            .duped_err()
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use indoc::indoc;

    use crate::legacy_configs;
    use crate::target_aliases::AliasResolutionError;
    use crate::target_aliases::BuckConfigTargetAliasResolver;

    #[test]
    fn test_aliases() -> buck2_error::Result<()> {
        let config = legacy_configs::configs::testing::parse(
            &[(
                "config",
                indoc!(
                    r#"
            [alias]
              baz = foo
              foo = //:foo
              bar = foo
              bar2 = bar
              cycle1 = cycle2
              cycle2 = cycle3
              cycle3 = cycle1
              chain1 = chain2
              chain2 = chain3

        "#
                ),
            )],
            "config",
        )?;

        let target_alias_resolver = BuckConfigTargetAliasResolver::new(config);

        assert_eq!("//:foo", target_alias_resolver.resolve_alias("foo")?);
        assert_eq!("//:foo", target_alias_resolver.resolve_alias("bar")?);
        assert_eq!("//:foo", target_alias_resolver.resolve_alias("bar2")?);
        assert_eq!("//:foo", target_alias_resolver.resolve_alias("baz")?);

        assert_matches!(
            target_alias_resolver.resolve_alias("missing"),
            Err(AliasResolutionError::NotAnAlias)
        );

        assert_matches!(
            target_alias_resolver.resolve_alias("chain1"),
            Err(e) => {
                let err = format!("{e:#}");
                let expected = "chain1 -> chain2 -> chain3";
                assert!(
                    err.contains(expected),
                    "expected error to contain `{expected}`, got `{err}`"
                );
            }
        );

        assert_matches!(
            target_alias_resolver.resolve_alias("cycle1"),
            Err(e) => {
                let err = format!("{e:#}");
                let expected = "cycle1 -> cycle2 -> cycle3 -> cycle1";
                assert!(
                    err.contains(expected),
                    "expected error to contain `{expected}`, got `{err}`"
                );
            }
        );

        Ok(())
    }
}
