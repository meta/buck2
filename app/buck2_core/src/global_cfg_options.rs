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

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;

use crate::target::label::label::TargetLabel;

#[derive(
    Default,
    Debug,
    Dupe,
    Clone,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
pub struct GlobalCfgOptions {
    pub target_platform: Option<TargetLabel>,
    // TODO(azhang2542): Replace with `Modifiers` struct
    pub cli_modifiers: Arc<Vec<String>>,
}
