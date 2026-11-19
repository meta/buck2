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
use buck2_core::bxl::BxlFilePath;
use buck2_core::bzl::ImportPath;
use pagable::Pagable;
use strong_hash::StrongHash;

#[derive(
    Debug,
    Clone,
    derive_more::Display,
    Eq,
    PartialEq,
    Hash,
    StrongHash,
    Pagable,
    Allocative
)]
#[display("{}", _0)]
pub enum BzlOrBxlPath {
    // bxl anon rule can be defined in bxl file
    Bxl(BxlFilePath),
    Bzl(ImportPath),
}
