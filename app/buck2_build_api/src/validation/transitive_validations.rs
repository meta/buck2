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
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_hash::BuckFixedSortedSet;
use dupe::Dupe;
use pagable::Pagable;

use crate::interpreter::rule_defs::provider::builtin::validation_info::OwnedValidationInfo;

/// Efficiently encoded collection of `ValidationInfo` providers for a given target node
/// and all of its recursive dependencies. Forms an optimized/sparse graph tracking
/// only those providers.
#[derive(Debug, Allocative, Dupe, Clone, Pagable)]
pub struct TransitiveValidations(pub Arc<TransitiveValidationsData>);

#[derive(Debug, Allocative, Pagable)]
pub struct TransitiveValidationsData {
    /// `ValidationInfo` provider if the current node contains it
    pub info: Option<OwnedValidationInfo>,
    /// If empty it means that there are no transitive dependencies of current node
    /// which contain `ValidationInfo` providers.
    pub children: BuckFixedSortedSet<ConfiguredTargetLabel>,
}
