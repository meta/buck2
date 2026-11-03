/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Dice operations for legacy configuration

use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::cells::cell_root_path::CellRootPathBuf;
use buck2_core::cells::name::CellName;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_events::dispatch::get_dispatcher;
use buck2_settings::buckconfig::BuckconfigSource;
use derive_more::Display;
use dice::DiceComputations;
use dice::DiceProjectionComputations;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::Key;
use dice::NoValueSerialize;
use dice::OkPagableValueSerialize;
use dice::OpaqueValue;
use dice::PagableValueSerialize;
use dice::ProjectionKey;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::dice::cells::HasCellResolver;
use crate::dice::data::HasIoProvider;
use crate::legacy_configs::cells::BuckConfigBasedCells;
use crate::legacy_configs::cells::ExternalBuckconfigData;
use crate::legacy_configs::cells::ExternalBuckconfigDataDiff;
use crate::legacy_configs::configs::LegacyBuckConfig;
use crate::legacy_configs::file_ops::ConfigParserFileOps;
use crate::legacy_configs::file_ops::DefaultConfigParserFileOps;
use crate::legacy_configs::key::BuckconfigKeyRef;
use crate::legacy_configs::view::LegacyBuckConfigView;

/// Buckconfig view which queries buckconfig entry from DICE.
#[derive(Clone, Dupe)]
pub struct OpaqueLegacyBuckConfigOnDice<'d> {
    config: Arc<OpaqueValue<'d, LegacyBuckConfigForCellKey>>,
}

impl<'d> std::fmt::Debug for OpaqueLegacyBuckConfigOnDice<'d> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacyBuckConfigOnDice")
            .field("config", &self.config)
            .finish()
    }
}

impl<'d> OpaqueLegacyBuckConfigOnDice<'d> {
    pub fn lookup(
        &self,
        ctx: &mut DiceComputations<'d>,
        key: BuckconfigKeyRef,
    ) -> buck2_error::Result<Option<Arc<str>>> {
        let BuckconfigKeyRef { section, property } = key;
        Ok(ctx.projection(
            &*self.config,
            &LegacyBuckConfigPropertyProjectionKey {
                section: section.to_owned(),
                property: property.to_owned(),
            },
        )?)
    }

    pub fn view<'a>(&'a self, ctx: &'a mut DiceComputations<'d>) -> LegacyBuckConfigOnDice<'a, 'd> {
        LegacyBuckConfigOnDice { ctx, config: self }
    }
}

pub struct LegacyBuckConfigOnDice<'a, 'd> {
    ctx: &'a mut DiceComputations<'d>,
    config: &'a OpaqueLegacyBuckConfigOnDice<'d>,
}

impl LegacyBuckConfigOnDice<'_, '_> {
    pub fn parse<T: FromStr>(&mut self, key: BuckconfigKeyRef) -> buck2_error::Result<Option<T>>
    where
        buck2_error::Error: From<<T as FromStr>::Err>,
    {
        LegacyBuckConfig::parse_value(key, LegacyBuckConfigView::get(self, key)?.as_deref())
    }
}

impl std::fmt::Debug for LegacyBuckConfigOnDice<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacyBuckConfigOnDice")
            .field("config", &self.config)
            .finish()
    }
}

impl LegacyBuckConfigView for LegacyBuckConfigOnDice<'_, '_> {
    fn get(&mut self, key: BuckconfigKeyRef) -> buck2_error::Result<Option<Arc<str>>> {
        self.config.lookup(self.ctx, key)
    }
}

/// A setting's buckconfig fallback read inside DICE depends on that key like
/// any other tracked buckconfig read.
impl BuckconfigSource for LegacyBuckConfigOnDice<'_, '_> {
    fn get(&mut self, section: &str, property: &str) -> buck2_error::Result<Option<String>> {
        Ok(self
            .config
            .lookup(self.ctx, BuckconfigKeyRef { section, property })?
            .map(|value| value.to_string()))
    }
}

pub trait HasInjectedLegacyConfigs<'d> {
    fn get_injected_external_buckconfig_data(
        &mut self,
    ) -> impl Future<Output = buck2_error::Result<&'d ExternalBuckconfigData>>;

    fn is_injected_external_buckconfig_data_key_set(
        &mut self,
    ) -> impl Future<Output = buck2_error::Result<bool>>;

    /// Describes how injecting `new` would change the injected external buckconfig data, or
    /// returns `None` if it would not. See `external_buckconfig_data_change`.
    fn injected_external_buckconfig_data_change(
        &mut self,
        new: &ExternalBuckconfigData,
    ) -> impl Future<Output = buck2_error::Result<Option<ExternalBuckconfigDataDiff>>>;

    /// Whether injecting `new` would change a resolved value of the root cell's buckconfig, rather
    /// than only the args that produce it. Returns `false` before the first injection. See
    /// `root_buckconfig_values_change`.
    fn injected_root_buckconfig_values_change(
        &mut self,
        new: &ExternalBuckconfigData,
    ) -> impl Future<Output = buck2_error::Result<bool>>;
}

/// Describes how injecting `new` would change `current`, the injected external buckconfig data,
/// or returns `None` if DICE would see the same value.
///
/// This uses the setter's filter (`filter_for_dice`) and the key's equality, so `Some` means
/// exactly that injecting `new` invalidates buckconfig-dependent DICE state; keep it in sync with
/// `LegacyExternalBuckConfigDataKey::equality_behavior`. `current` is `None` until the first
/// command injects configs, and then there is no state to invalidate.
fn external_buckconfig_data_change(
    current: Option<&ExternalBuckconfigData>,
    new: &ExternalBuckconfigData,
) -> Option<ExternalBuckconfigDataDiff> {
    let current = current?;
    let new = filter_for_dice(new.clone());
    if *current == new {
        None
    } else {
        Some(current.diff(&new))
    }
}

/// Whether injecting `new` instead of `current` changes a resolved value of the root cell's
/// buckconfig.
///
/// Both configs are parsed the way `LegacyBuckConfigForCellKey` parses a cell, including
/// `<file:...>` includes, from the same repo files, so only the external data differs. Dropping
/// `-c foo.a=2` where the repo already sets `foo.a = 2` is then no change. Comparing against the
/// `root_config` that `BuckConfigBasedCells::parse_with_config_args` returns would be wrong: it
/// does not follow includes. Parsing the repo config twice is acceptable because callers only ask
/// after a cheaper check finds dropped args.
async fn root_buckconfig_values_change(
    file_ops: &mut dyn ConfigParserFileOps,
    current: &ExternalBuckconfigData,
    new: &ExternalBuckconfigData,
) -> buck2_error::Result<bool> {
    let new = filter_for_dice(new.clone());
    let root = CellRootPathBuf::new(ProjectRelativePath::empty().to_owned());
    let current =
        BuckConfigBasedCells::parse_single_cell_with_file_ops_inner(current, file_ops, &root)
            .await?;
    let new =
        BuckConfigBasedCells::parse_single_cell_with_file_ops_inner(&new, file_ops, &root).await?;
    Ok(!current.compare(&new))
}

#[async_trait]
pub trait HasLegacyConfigs<'d> {
    /// Get buckconfigs.
    ///
    /// This operation does not record buckconfig as a dependency of current computation.
    /// Accessing specific buckconfig property, records that key as dependency.
    async fn get_legacy_config_on_dice(
        &mut self,
        cell_name: CellName,
    ) -> buck2_error::Result<OpaqueLegacyBuckConfigOnDice<'d>>;

    async fn get_legacy_root_config_on_dice(
        &mut self,
    ) -> buck2_error::Result<OpaqueLegacyBuckConfigOnDice<'d>>;

    /// Use this function carefully: a computation which fetches this key will be recomputed
    /// if any buckconfig property changes.
    ///
    /// Consider using `get_legacy_config_property` instead.
    async fn get_legacy_config_for_cell(
        &mut self,
        cell_name: CellName,
    ) -> buck2_error::Result<&'d LegacyBuckConfig>;

    async fn get_legacy_config_property(
        &mut self,
        cell_name: CellName,
        key: BuckconfigKeyRef<'_>,
    ) -> buck2_error::Result<Option<Arc<str>>>;

    async fn parse_legacy_config_property<T: FromStr>(
        &mut self,
        cell_name: CellName,
        key: BuckconfigKeyRef<'_>,
    ) -> buck2_error::Result<Option<T>>
    where
        buck2_error::Error: From<<T as FromStr>::Err>,
        T: Send + Sync + 'static;

    async fn parse_legacy_config_list_property<T: FromStr>(
        &mut self,
        cell_name: CellName,
        key: BuckconfigKeyRef<'_>,
    ) -> buck2_error::Result<Option<Vec<T>>>
    where
        buck2_error::Error: From<<T as FromStr>::Err>,
        T: Send + Sync + 'static;
}

pub trait SetLegacyConfigs {
    fn set_legacy_config_external_data(
        &mut self,
        overrides: ExternalBuckconfigData,
    ) -> buck2_error::Result<()>;

    fn set_none_legacy_config_external_data(&mut self) -> buck2_error::Result<()>;
}

#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct LegacyExternalBuckConfigDataKey;

impl InjectedKey for LegacyExternalBuckConfigDataKey {
    type Value = Option<ExternalBuckconfigData>;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

#[derive(Clone, Display, Debug, Hash, Eq, PartialEq, Allocative, Pagable)]
#[display("LegacyBuckConfigForCellKey({})", self.cell_name)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct LegacyBuckConfigForCellKey {
    cell_name: CellName,
}

#[async_trait]
impl Key for LegacyBuckConfigForCellKey {
    type Value = buck2_error::Result<LegacyBuckConfig>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> buck2_error::Result<LegacyBuckConfig> {
        let cells = ctx.get_cell_resolver().await?;
        let this_cell = cells.get(self.cell_name)?;
        let config = BuckConfigBasedCells::parse_single_cell_with_dice(ctx, this_cell.path())
            .await
            .with_buck_error_context(|| {
                format!("Computing legacy buckconfigs for cell `{}`", self.cell_name)
            })?;
        let config = config.filter_values(is_config_invisible_to_dice);

        let event = buck2_data::CellHasNewConfigs {
            cell: self.cell_name.as_str().to_owned(),
        };
        get_dispatcher().instant_event(event);

        Ok(config)
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x.compare(y),
            _ => false,
        })
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

/// The computation `LegacyBuckConfigForCellKey` computation might encounter an error.
///
/// We can't return that error immediately, because we only compute the opaque value. We could
/// return the error when doing the projection to the buckconfig values, but that would result in us
/// increasing the size of the value returned from that computation. Instead, we'll use a different
/// projection key to extract just the error from the cell computation, and compute that when
/// constructing the `OpaqueLegacyBuckConfigOnDice`.
#[derive(Debug, Display, Hash, Eq, PartialEq, Clone, Allocative, Pagable)]
#[pagable_typetag(dice::DiceProjectionDyn)]
struct LegacyBuckConfigErrorKey();

impl ProjectionKey for LegacyBuckConfigErrorKey {
    type DeriveFromKey = LegacyBuckConfigForCellKey;
    type Value = Option<buck2_error::Error>;

    fn compute(
        &self,
        config: &buck2_error::Result<LegacyBuckConfig>,
        _ctx: &DiceProjectionComputations,
    ) -> Option<buck2_error::Error> {
        config.as_ref().err().cloned()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x.is_none() && y.is_none())
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[derive(Debug, Display, Hash, Eq, PartialEq, Clone, Allocative, Pagable)]
#[display("{}.{}", section, property)]
#[pagable_typetag(dice::DiceProjectionDyn)]
struct LegacyBuckConfigPropertyProjectionKey {
    section: String,
    property: String,
}

impl ProjectionKey for LegacyBuckConfigPropertyProjectionKey {
    type DeriveFromKey = LegacyBuckConfigForCellKey;
    type Value = Option<Arc<str>>;

    fn compute(
        &self,
        config: &buck2_error::Result<LegacyBuckConfig>,
        _ctx: &DiceProjectionComputations,
    ) -> Option<Arc<str>> {
        // See the comment in `LegacyBuckConfigErrorKey` for why this is safe
        let config = config.as_ref().unwrap();
        config
            .get(BuckconfigKeyRef {
                section: &self.section,
                property: &self.property,
            })
            .map(|s| s.to_owned().into())
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

impl<'d> HasInjectedLegacyConfigs<'d> for DiceComputations<'d> {
    async fn get_injected_external_buckconfig_data(
        &mut self,
    ) -> buck2_error::Result<&'d ExternalBuckconfigData> {
        self.compute(&LegacyExternalBuckConfigDataKey).await?.as_ref().internal_error("Tried to retrieve LegacyExternalBuckConfigDataKey from the graph, but key has None value")
    }

    async fn is_injected_external_buckconfig_data_key_set(&mut self) -> buck2_error::Result<bool> {
        Ok(self
            .compute(&LegacyExternalBuckConfigDataKey)
            .await?
            .is_some())
    }

    async fn injected_external_buckconfig_data_change(
        &mut self,
        new: &ExternalBuckconfigData,
    ) -> buck2_error::Result<Option<ExternalBuckconfigDataDiff>> {
        let current = self.compute(&LegacyExternalBuckConfigDataKey).await?;
        Ok(external_buckconfig_data_change(current.as_ref(), new))
    }

    async fn injected_root_buckconfig_values_change(
        &mut self,
        new: &ExternalBuckconfigData,
    ) -> buck2_error::Result<bool> {
        let project_fs = self.global_data().get_io_provider().project_root().dupe();
        let Some(current) = self.compute(&LegacyExternalBuckConfigDataKey).await? else {
            return Ok(false);
        };
        // Read the repo files from disk rather than through DICE: both sides must see the same
        // files, and this state's view is about to be replaced by the file watcher sync anyway.
        root_buckconfig_values_change(&mut DefaultConfigParserFileOps { project_fs }, current, new)
            .await
    }
}

pub fn inject_legacy_config_for_test(
    dice: &mut DiceTransactionUpdater,
    cell_name: CellName,
    configs: LegacyBuckConfig,
) -> buck2_error::Result<()> {
    inject_legacy_configs_for_test(dice, [(cell_name, configs)])
}

pub fn inject_legacy_configs_for_test(
    dice: &mut DiceTransactionUpdater,
    configs: impl IntoIterator<Item = (CellName, LegacyBuckConfig)> + Send + Sync + 'static,
) -> buck2_error::Result<()> {
    let configs = configs
        .into_iter()
        .map(|(cell_name, configs)| (LegacyBuckConfigForCellKey { cell_name }, Ok(configs)))
        .collect::<Vec<_>>();
    dice.changed_to(configs)?;
    dice.changed_to([(LegacyExternalBuckConfigDataKey, None)])?;
    Ok(())
}

#[async_trait]
impl<'d> HasLegacyConfigs<'d> for DiceComputations<'d> {
    async fn get_legacy_config_on_dice(
        &mut self,
        cell_name: CellName,
    ) -> buck2_error::Result<OpaqueLegacyBuckConfigOnDice<'d>> {
        let config = self
            .compute_opaque(&LegacyBuckConfigForCellKey { cell_name })
            .await?;
        if let Some(error) = self.projection(&config, &LegacyBuckConfigErrorKey())? {
            return Err(error);
        }
        Ok(OpaqueLegacyBuckConfigOnDice {
            config: Arc::new(config),
        })
    }

    async fn get_legacy_root_config_on_dice(
        &mut self,
    ) -> buck2_error::Result<OpaqueLegacyBuckConfigOnDice<'d>> {
        let cell_resolver = self.get_cell_resolver().await?;
        self.get_legacy_config_on_dice(cell_resolver.root_cell())
            .await
    }

    async fn get_legacy_config_for_cell(
        &mut self,
        cell_name: CellName,
    ) -> buck2_error::Result<&'d LegacyBuckConfig> {
        self.compute(&LegacyBuckConfigForCellKey { cell_name })
            .await?
            .as_ref()
            .duped_err()
    }

    async fn get_legacy_config_property(
        &mut self,
        cell_name: CellName,
        key: BuckconfigKeyRef<'_>,
    ) -> buck2_error::Result<Option<Arc<str>>> {
        self.get_legacy_config_on_dice(cell_name)
            .await?
            .lookup(self, key)
    }

    async fn parse_legacy_config_property<T: FromStr>(
        &mut self,
        cell_name: CellName,
        key: BuckconfigKeyRef<'_>,
    ) -> buck2_error::Result<Option<T>>
    where
        buck2_error::Error: From<<T as FromStr>::Err>,
        T: Send + Sync + 'static,
    {
        LegacyBuckConfig::parse_value(
            key,
            self.get_legacy_config_property(cell_name, key)
                .await?
                .as_deref(),
        )
    }

    async fn parse_legacy_config_list_property<T: FromStr>(
        &mut self,
        cell_name: CellName,
        key: BuckconfigKeyRef<'_>,
    ) -> buck2_error::Result<Option<Vec<T>>>
    where
        buck2_error::Error: From<<T as FromStr>::Err>,
        T: Send + Sync + 'static,
    {
        LegacyBuckConfig::parse_list_value(
            key,
            self.get_legacy_config_property(cell_name, key)
                .await?
                .as_deref(),
        )
    }
}

impl SetLegacyConfigs for DiceTransactionUpdater {
    fn set_legacy_config_external_data(
        &mut self,
        data: ExternalBuckconfigData,
    ) -> buck2_error::Result<()> {
        Ok(self.changed_to(vec![(
            LegacyExternalBuckConfigDataKey,
            Some(filter_for_dice(data)),
        )])?)
    }

    fn set_none_legacy_config_external_data(&mut self) -> buck2_error::Result<()> {
        Ok(self.changed_to(vec![(LegacyExternalBuckConfigDataKey, None)])?)
    }
}

/// The form in which external buckconfig data is injected into DICE. Shared by the setter and
/// `external_buckconfig_data_change` so the change check cannot disagree with what is injected.
fn filter_for_dice(data: ExternalBuckconfigData) -> ExternalBuckconfigData {
    data.filter_values(is_config_invisible_to_dice)
}

fn is_config_invisible_to_dice(key: &BuckconfigKeyRef) -> bool {
    !CONFIGS_INVISIBLE_TO_DICE.contains(key)
}

/// A set of buckconfigs that are visibile outside of dice, but not within it. Importantly, changes
/// to these configs do not cause state invalidations.
// FIXME(JakobDegen): Error if someone tries to read any of these from in dice
const CONFIGS_INVISIBLE_TO_DICE: &[BuckconfigKeyRef<'static>] = &[
    BuckconfigKeyRef {
        section: "buck2_re_client",
        property: "override_use_case",
    },
    BuckconfigKeyRef {
        section: "scuba",
        property: "defaults",
    },
];

#[cfg(test)]
mod tests {
    use buck2_cli_proto::ConfigOverride;

    use super::external_buckconfig_data_change;
    use super::filter_for_dice;
    use super::root_buckconfig_values_change;
    use crate::legacy_configs::cells::BuckConfigBasedCells;
    use crate::legacy_configs::cells::ExternalBuckconfigData;
    use crate::legacy_configs::configs::testing::TestConfigParserFileOps;
    use crate::legacy_configs::configs::testing::parse_with_config_args;

    /// The external data a command passing these `--config` flags would inject.
    async fn external_data(flags: &[&str]) -> buck2_error::Result<ExternalBuckconfigData> {
        let mut file_ops = TestConfigParserFileOps::new(&[(".buckconfig", "[cells]\nroot = .\n")])?;
        let args: Vec<_> = flags
            .iter()
            .map(|flag| ConfigOverride::flag_no_cell(flag))
            .collect();
        Ok(
            BuckConfigBasedCells::testing_parse_with_file_ops(&mut file_ops, &args)
                .await?
                .external_data,
        )
    }

    /// The value a previous command passing these flags left in DICE.
    async fn injected(flags: &[&str]) -> buck2_error::Result<ExternalBuckconfigData> {
        Ok(filter_for_dice(external_data(flags).await?))
    }

    #[tokio::test]
    async fn no_change_before_first_injection() -> buck2_error::Result<()> {
        let new = external_data(&["foo.bar=baz"]).await?;
        assert_eq!(external_buckconfig_data_change(None, &new), None);
        Ok(())
    }

    #[tokio::test]
    async fn same_flags_do_not_change() -> buck2_error::Result<()> {
        let current = injected(&["foo.bar=baz"]).await?;
        let new = external_data(&["foo.bar=baz"]).await?;
        assert_eq!(external_buckconfig_data_change(Some(&current), &new), None);
        Ok(())
    }

    #[tokio::test]
    async fn flag_changes_are_reported() -> buck2_error::Result<()> {
        let current = injected(&["foo.bar=baz"]).await?;

        // A command that omits the flag, like `uquery` after `build -c foo.bar=baz`.
        let change =
            external_buckconfig_data_change(Some(&current), &external_data(&[]).await?).unwrap();
        assert_eq!(change.removed_args, vec!["--config foo.bar=baz"]);
        assert!(change.added_args.is_empty());

        let change = external_buckconfig_data_change(
            Some(&current),
            &external_data(&["foo.bar=qux"]).await?,
        )
        .unwrap();
        assert_eq!(change.added_args, vec!["--config foo.bar=qux"]);
        assert_eq!(change.removed_args, vec!["--config foo.bar=baz"]);

        let change =
            external_buckconfig_data_change(Some(&current), &external_data(&["foo.bar="]).await?)
                .unwrap();
        assert_eq!(change.added_args, vec!["--config foo.bar="]);

        Ok(())
    }

    #[tokio::test]
    async fn configs_invisible_to_dice_do_not_change() -> buck2_error::Result<()> {
        let current = injected(&[]).await?;
        let new = external_data(&["scuba.defaults=x"]).await?;
        assert_eq!(external_buckconfig_data_change(Some(&current), &new), None);
        Ok(())
    }

    #[tokio::test]
    async fn root_values_change_only_when_dropped_args_set_other_values() -> buck2_error::Result<()>
    {
        // The default comes through an include, which the comparison must follow.
        let files = [
            (".buckconfig", "[cells]\nroot = .\n<file:defaults.bcfg>\n"),
            ("defaults.bcfg", "[foo]\ndefault = 2\n"),
        ];
        for (current, new, expected) in [
            // Dropping the override returns `foo.default` to the repo's value.
            (&["foo.default=1"][..], &[][..], true),
            // The dropped arg restated the repo's value.
            (&["foo.default=2"], &[], false),
            // The dropped arg set a key the repo does not set.
            (&["foo.other=1"], &[], true),
            (&[], &["scuba.defaults=x"], false),
        ] {
            let mut file_ops = TestConfigParserFileOps::new(&files)?;
            let changed = root_buckconfig_values_change(
                &mut file_ops,
                &injected(current).await?,
                &external_data(new).await?,
            )
            .await?;
            assert_eq!(changed, expected, "{current:?} -> {new:?}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn reordered_flags_change_with_empty_diff() -> buck2_error::Result<()> {
        // Later flags override earlier ones, so DICE compares args in order: reordering the same
        // flags still invalidates, even though no flag was added or removed.
        let current = injected(&["a.b=1", "c.d=2"]).await?;
        let new = external_data(&["c.d=2", "a.b=1"]).await?;
        let change = external_buckconfig_data_change(Some(&current), &new).unwrap();
        assert!(change.is_empty());
        Ok(())
    }

    #[test]
    fn config_equals() -> buck2_error::Result<()> {
        let path = "test";
        let config1 = parse_with_config_args(
            &[("test", "[sec1]\na=b\n[sec2]\nx=y")],
            path,
            &[ConfigOverride::flag_no_cell("sec1.a=c")],
        )?;

        let config2 = parse_with_config_args(&[("test", "[sec1]\na=c\n[sec2]\nx=y")], path, &[])?;

        let config3 = parse_with_config_args(
            &[("test", "[sec1]\na=b\n[sec2]\nx=y")],
            path,
            &[ConfigOverride::flag_no_cell("sec1.d=e")],
        )?;

        assert!(config1.compare(&config1));
        assert!(config2.compare(&config2));
        assert!(config3.compare(&config3));
        assert!(config1.compare(&config2));
        assert!(!config1.compare(&config3));
        assert!(!config2.compare(&config3));

        Ok(())
    }
}
