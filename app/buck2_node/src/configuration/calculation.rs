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
use buck2_core::cells::name::CellName;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::target::label::label::TargetLabel;
use buck2_util::late_binding::LateBinding;
use derive_more::Display;
use dice::DiceComputations;
use dupe::Dupe;

#[async_trait]
pub trait ConfigurationCalculationDyn: Send + Sync + 'static {
    async fn get_platform_configuration(
        &self,
        dice: &mut DiceComputations<'_>,
        target: &TargetLabel,
    ) -> buck2_error::Result<ConfigurationData>;
}

/// For config_settings that need to be resolved when producing a ResolvedConfiguration, the buckconfig values are looked up in
/// the cell that the configuration is resolving in. This means that for selects that appear in a target, the config_settings in the keys
/// would resolve based on the buckconfigs from that target's cell.
///
/// This is subtle, non-obvious and possibly unintuitive, so we introduce a newtype here just to make it clearer in the places we are
/// using or passing around a CellName for this purpose.
#[derive(
    Clone,
    Dupe,
    Copy,
    Debug,
    Display,
    Hash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Allocative,
    pagable::Pagable
)]
pub struct CellNameForConfigurationResolution(pub CellName);

pub static CONFIGURATION_CALCULATION: LateBinding<&'static dyn ConfigurationCalculationDyn> =
    LateBinding::new("CONFIGURATION_CALCULATION");
