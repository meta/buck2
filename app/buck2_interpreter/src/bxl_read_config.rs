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

use buck2_util::late_binding::LateBinding;
use starlark::eval::Evaluator;

/// Result of trying to read a buckconfig value while evaluating a `.bxl` script,
/// where there is no `BuildContext`.
pub enum BxlConfigValue {
    /// The current evaluation is not a BXL evaluation; the caller should fall
    /// through to its normal handling.
    NotBxl,
    /// A BXL evaluation. The value is the config value, or `None` if unset.
    Bxl(Option<Arc<str>>),
}

/// Reads a buckconfig value from the cell that owns the `.bxl` file while a BXL
/// script is being evaluated.
///
/// `read_config` should work when evaluated in a BXL context, using the BXL file's
/// cell for buckconfig lookups. This hook, implemented in `buck2_bxl`, lets it
/// work there by reading straight from DICE.
pub static BXL_READ_CONFIG: LateBinding<
    fn(&mut Evaluator, &str, &str) -> buck2_error::Result<BxlConfigValue>,
> = LateBinding::new("BXL_READ_CONFIG");
