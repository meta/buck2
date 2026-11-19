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
use buck2_settings_derive::buck_settings_section;
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;

/// Controls whether ordinary `package(visibility=...)` declarations
/// contribute to the visibility intersection by default.
///
/// * `off` (default): legacy behavior; only `enforce_visibility_intersection()`
///   boundaries contribute.
/// * `audit`: the default intersection is shadow-evaluated; would-block edges
///   are reported, not fatal.
/// * `enforce`: ordinary PACKAGE visibility is enforced by default; the marker
///   is redundant.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Allocative,
    Pagable
)]
#[serde(rename_all = "snake_case")]
pub enum PackageVisibilityDefaultIntersection {
    #[default]
    Off,
    Audit,
    Enforce,
}

/// Settings controlling PACKAGE visibility intersection.
#[buck_settings_section(name = "package_visibility", version = 0, scope = daemon)]
pub struct PackageVisibility {
    /// Whether ordinary `package(visibility=...)` declarations contribute to
    /// the visibility intersection: `off`, `audit` or `enforce`.
    // Enforcement control: overrides must not be able to weaken it.
    #[setting(
        default = PackageVisibilityDefaultIntersection::Off,
        overridable_in()
    )]
    default_intersection: Option<PackageVisibilityDefaultIntersection>,
}
