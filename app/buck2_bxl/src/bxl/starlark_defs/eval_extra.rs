/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;
use std::sync::Arc;

use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_events::dispatch::console_message;
use buck2_interpreter::bxl_read_config::BXL_READ_CONFIG;
use buck2_interpreter::bxl_read_config::BxlConfigValue;
use futures::FutureExt;
use starlark::eval::Evaluator;
use starlark::values::ProvidesStaticType;

use crate::bxl::starlark_defs::context::BxlContextCoreData;
use crate::bxl::starlark_defs::context::ErrorPrinter;
use crate::bxl::starlark_defs::context::output::OutputStreamState;
use crate::bxl::starlark_defs::context::starlark_async::BxlDiceComputations;

enum BxlEvalExtraType {
    Root { stream_state: OutputStreamState },
    Dynamic,
    AnonTarget,
}

/// A tag that is only available when running in Bxl, to guard Bxl
/// functions from a non-Bxl context.
#[derive(ProvidesStaticType)]
pub(crate) struct BxlEvalExtra<'d> {
    pub(crate) dice: BxlDiceComputations<'d>,
    pub(crate) core: Arc<BxlContextCoreData>,
    eval_extra_type: BxlEvalExtraType,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub(crate) enum BxlScopeError {
    #[error("This function can only be called from Bxl")]
    UnavailableOutsideBxl,
}

impl<'d> BxlEvalExtra<'d> {
    pub(crate) fn new(
        dice: BxlDiceComputations<'d>,
        core: Arc<BxlContextCoreData>,
        stream_state: OutputStreamState,
    ) -> Self {
        Self {
            dice,
            core,
            eval_extra_type: BxlEvalExtraType::Root { stream_state },
        }
    }

    pub(crate) fn new_dynamic(
        dice: BxlDiceComputations<'d>,
        core: Arc<BxlContextCoreData>,
    ) -> Self {
        Self {
            dice,
            core,
            eval_extra_type: BxlEvalExtraType::Dynamic,
        }
    }

    pub(crate) fn new_anon(dice: BxlDiceComputations<'d>, core: Arc<BxlContextCoreData>) -> Self {
        Self {
            dice,
            core,
            eval_extra_type: BxlEvalExtraType::AnonTarget,
        }
    }

    pub(crate) fn from_context<'s, 'v, 'a>(
        eval: &'s mut Evaluator<'v, 'a, 'd>,
    ) -> buck2_error::Result<&'s mut BxlEvalExtra<'d>> {
        match &mut eval.extra_mut {
            Some(extra) => extra.downcast_mut::<BxlEvalExtra>(),
            None => None,
        }
        .ok_or_else(|| BxlScopeError::UnavailableOutsideBxl.into())
    }
}

/// Implementation of [`BXL_READ_CONFIG`]: read a buckconfig value from the cell
/// that owns the `.bxl`.
fn bxl_read_config(
    eval: &mut Evaluator,
    section: &str,
    key: &str,
) -> buck2_error::Result<BxlConfigValue> {
    let extra = match BxlEvalExtra::from_context(eval) {
        Ok(extra) => extra,
        // Not a BXL evaluation; let the caller fall through to its own handling.
        Err(_) => return Ok(BxlConfigValue::NotBxl),
    };

    let cell = extra.core.cell_name();
    let section = section.to_owned();
    let key = key.to_owned();
    let value = extra.dice.via(move |dice| {
        async move {
            dice.get_legacy_config_property(
                cell,
                BuckconfigKeyRef {
                    section: &section,
                    property: &key,
                },
            )
            .await
        }
        .boxed_local()
    })?;

    Ok(BxlConfigValue::Bxl(value))
}

pub(crate) fn init_bxl_read_config() {
    BXL_READ_CONFIG.init(bxl_read_config);
}

impl<'e> ErrorPrinter for BxlEvalExtra<'e> {
    fn print_to_error_stream(&self, msg: String) -> buck2_error::Result<()> {
        match &self.eval_extra_type {
            BxlEvalExtraType::Root { stream_state } => writeln!(stream_state.error(), "{msg}")?,
            BxlEvalExtraType::Dynamic => console_message(msg),
            BxlEvalExtraType::AnonTarget => console_message(msg),
        }
        Ok(())
    }
}
