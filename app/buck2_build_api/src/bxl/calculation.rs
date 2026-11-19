/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! DICE calculations for bxl

use std::fmt::Debug;
use std::sync::Arc;

use buck2_core::deferred::base_deferred_key::BaseDeferredKeyBxl;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use futures::future::BoxFuture;

use crate::bxl::result::BxlResult;

pub trait BxlCalculationDyn: Debug + Send + Sync + 'static {
    fn eval_bxl<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
        bxl: BaseDeferredKeyBxl,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Arc<BxlResult>>>
    where
        'd: 'a;
}

/// Dependency injection for BXL.
///
/// BXL implementation lives in downstream crate.
/// This field is initialized at program start, so this crate can call BXL calculation.
pub static BXL_CALCULATION_IMPL: LateBinding<&'static dyn BxlCalculationDyn> =
    LateBinding::new("BXL_CALCULATION_IMPL");
