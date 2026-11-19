/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::iter;

use allocative::Allocative;
use buck2_core::package::package_relative_path::PackageRelativePath;
use buck2_util::arc_str::PackedStr;
use buck2_util::size_assert;
use either::Either;
use pagable::Pagable;
use strong_hash::StrongHash;

#[derive(Debug, Eq, PartialEq, Hash, Clone, Allocative, Pagable, StrongHash)]
pub struct CoercedDirectory {
    pub dir: PackedStr<PackageRelativePath>,
    // We can make this type DST, so there would be only one allocation
    // for directory itself and files. But we don't have a lot of directories,
    // so it is not worth the trouble.
    pub files: Box<[PackedStr<PackageRelativePath>]>,
}

#[derive(Debug, Eq, PartialEq, Hash, Clone, Allocative, Pagable, StrongHash)]
pub enum CoercedPath {
    File(PackedStr<PackageRelativePath>),
    Directory(Box<CoercedDirectory>),
}

// Avoid changing the size accidentally.
size_assert::words_of_type!(CoercedPath, 2);

impl CoercedPath {
    pub fn path(&self) -> &PackedStr<PackageRelativePath> {
        match self {
            CoercedPath::File(x) => x,
            CoercedPath::Directory(x) => &x.dir,
        }
    }

    pub fn inputs(&self) -> impl Iterator<Item = &'_ PackedStr<PackageRelativePath>> {
        match self {
            CoercedPath::File(x) => Either::Left(iter::once(x)),
            CoercedPath::Directory(x) => Either::Right(x.files.iter()),
        }
    }
}
