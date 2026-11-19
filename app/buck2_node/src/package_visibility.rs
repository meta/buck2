/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Per-transaction plumbing for the `package_visibility.default_intersection` Buck setting.
//!
//! The mode is fixed for the daemon's lifetime (changing the checked-in setting
//! restarts the daemon), so it travels as per-transaction data rather than a
//! DICE key — no invalidation is ever needed.

use buck2_settings::PackageVisibilityDefaultIntersection;
use dice::UserComputationData;

/// Per-transaction storage for the effective mode.
pub trait HasPackageVisibilityDefaultIntersection {
    /// Called once per command from `make_user_computation_data`.
    fn set_package_visibility_default_intersection(
        &mut self,
        mode: PackageVisibilityDefaultIntersection,
    );

    /// Panics if [`set_package_visibility_default_intersection`](Self::set_package_visibility_default_intersection)
    /// was not called — every production transaction sets it via
    /// `make_user_computation_data`.
    fn get_package_visibility_default_intersection(&self) -> PackageVisibilityDefaultIntersection;
}

impl HasPackageVisibilityDefaultIntersection for UserComputationData {
    fn set_package_visibility_default_intersection(
        &mut self,
        mode: PackageVisibilityDefaultIntersection,
    ) {
        self.data.set(mode);
    }

    fn get_package_visibility_default_intersection(&self) -> PackageVisibilityDefaultIntersection {
        self.data
            .get::<PackageVisibilityDefaultIntersection>()
            .copied()
            .expect("PackageVisibilityDefaultIntersection should be set")
    }
}
