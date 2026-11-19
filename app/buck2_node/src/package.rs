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
use buck2_core::build_file_path::BuildFilePath;
use pagable::Pagable;

use crate::oncall::Oncall;
use crate::visibility::VisibilityIntersection;

/// Package-specific data for `TargetNode`.
///
/// (Note this has nothing to do with `PACKAGE` files which are not implemented
/// at the moment of writing.)
#[derive(Debug, Hash, Allocative, Eq, PartialEq, Pagable)]
pub struct Package {
    /// The build file which defined this target, e.g. `fbcode//foo/bar/TARGETS`
    pub buildfile_path: Arc<BuildFilePath>,
    /// The oncall attribute, if set
    pub oncall: Option<Oncall>,
    /// Ancestor `PACKAGE` intersection, minus layers exempting this package
    /// (`VisibilityIntersection::for_defining_package`). Empty = no restriction.
    /// Stored once per build file; ANDed with `visibility` at `is_visible_to` time.
    pub visibility_intersection: VisibilityIntersection,
}
