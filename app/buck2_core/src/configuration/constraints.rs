/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Constraints are the building block of buck platforms.
//!
//! A constraint is identified by a "constraint key" defined by a `constraint_setting()`
//! target. There may be multiple possible values for the constraint, each defined by a
//! `constraint_value()` target.
//!
//! Currently, a platform or configuration has at most a single value for each constraint.

use allocative::Allocative;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

use crate::provider::label::ProvidersLabel;
use crate::provider::label::testing::ProvidersLabelTestExt;
use crate::target::label::label::TargetLabel;

/// A ConstraintKey is a label for a `constraint_setting()` target.
#[derive(
    Clone, Dupe, Debug, Display, Hash, Eq, PartialEq, Ord, PartialOrd, Allocative, StrongHash,
    Pagable
)]
#[display("{}", key)]
pub struct ConstraintKey {
    pub key: TargetLabel,
    // TODO(nero): remove Option when we migrated to unified constraint rule
    pub default: Option<ConstraintValue>,
}

impl ConstraintKey {
    pub fn testing_new(label: &str) -> ConstraintKey {
        ConstraintKey {
            key: TargetLabel::testing_parse(label),
            default: None,
        }
    }
}

#[derive(
    Clone, Dupe, Debug, Display, Hash, Eq, PartialEq, Ord, PartialOrd, Allocative, StrongHash,
    Pagable
)]
pub struct ConstraintValue(pub ProvidersLabel);

impl ConstraintValue {
    pub fn testing_new(label: &str, name: Option<&str>) -> ConstraintValue {
        let name_array = name.map(|n| [n]);
        ConstraintValue(ProvidersLabel::testing_new_with_target_label(
            TargetLabel::testing_parse(label),
            name_array.as_ref().map(|a| &a[..]),
        ))
    }
}
