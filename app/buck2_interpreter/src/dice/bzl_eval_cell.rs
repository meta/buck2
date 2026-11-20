/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_core::bzl::BzlEvalCell;
use dice::DiceComputations;

#[async_trait]
pub trait HasBzlEvalCell {
    /// `BzlEvalCell::Own` if the root cell sets `buck2.eval_bzl_in_own_cell`. One setting for
    /// the whole build, because it decides which evaluation of a file a `load()` refers to.
    async fn get_bzl_eval_cell(&mut self) -> buck2_error::Result<BzlEvalCell>;
}

#[async_trait]
impl HasBzlEvalCell for DiceComputations<'_> {
    async fn get_bzl_eval_cell(&mut self) -> buck2_error::Result<BzlEvalCell> {
        let root_cell = self.get_cell_resolver().await?.root_cell();
        let own_cell = self
            .parse_legacy_config_property::<bool>(
                root_cell,
                BuckconfigKeyRef {
                    section: "buck2",
                    property: "eval_bzl_in_own_cell",
                },
            )
            .await?
            .unwrap_or(false);
        Ok(if own_cell {
            BzlEvalCell::Own
        } else {
            BzlEvalCell::Loader
        })
    }
}
