/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_fs::paths::cmp_impls::impl_cmp;

use crate::fs::project_rel_path::ProjectRelativePath;
use crate::fs::project_rel_path::ProjectRelativePathBuf;
use crate::package::package_relative_path::PackageRelativePath;
use crate::package::package_relative_path::PackageRelativePathBuf;

impl_cmp!(
    PackageRelativePathBuf,
    PackageRelativePath,
    PackageRelativePath
);
impl_cmp!(
    PackageRelativePathBuf,
    &'_ PackageRelativePath,
    PackageRelativePath
);

impl_cmp!(
    ProjectRelativePathBuf,
    ProjectRelativePath,
    ProjectRelativePath
);
impl_cmp!(
    ProjectRelativePathBuf,
    &'_ ProjectRelativePath,
    ProjectRelativePath
);
