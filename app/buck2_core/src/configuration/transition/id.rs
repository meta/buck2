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
use derive_more::Display;
use pagable::Pagable;
use strong_hash::StrongHash;

use crate::bzl::ImportPath;
use crate::provider::label::ProvidersLabel;

/// Identifier of transition function.
#[derive(
    Debug, Clone, Hash, StrongHash, Eq, PartialEq, Ord, PartialOrd, Display, Allocative, Pagable
)]
pub enum TransitionId {
    #[display("{}#{}", path, name)]
    MagicObject { path: ImportPath, name: String },
    #[display("{}", _0)]
    Target(ProvidersLabel),
}
