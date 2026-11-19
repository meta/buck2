/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::paths::file_name::FileName;
use crate::paths::file_name::FileNameBuf;
use crate::paths::forward_rel_path::ForwardRelativePath;
use crate::paths::forward_rel_path::ForwardRelativePathBuf;

/// Provide an iterator of FileNameBuf from inputs that can produce one. This is useful for methods
/// that insert into directory mappings.
pub trait IntoFileNameBufIterator {
    type Iterator: Iterator<Item = FileNameBuf>;

    fn into_iter(self) -> Self::Iterator;
}

impl<'a> IntoFileNameBufIterator for &'a ForwardRelativePath {
    type Iterator = impl Iterator<Item = FileNameBuf> + 'a;

    fn into_iter(self) -> Self::Iterator {
        self.iter().map(FileName::to_owned)
    }
}

impl<'a> IntoFileNameBufIterator for &'a ForwardRelativePathBuf {
    type Iterator = impl Iterator<Item = FileNameBuf> + 'a;

    fn into_iter(self) -> Self::Iterator {
        self.iter().map(FileName::to_owned)
    }
}

impl IntoFileNameBufIterator for &FileName {
    type Iterator = impl Iterator<Item = FileNameBuf>;

    fn into_iter(self) -> Self::Iterator {
        std::iter::once(self.to_owned())
    }
}

impl IntoFileNameBufIterator for &FileNameBuf {
    type Iterator = impl Iterator<Item = FileNameBuf>;

    fn into_iter(self) -> Self::Iterator {
        std::iter::once(self.clone())
    }
}

impl IntoFileNameBufIterator for FileNameBuf {
    type Iterator = impl Iterator<Item = FileNameBuf>;

    fn into_iter(self) -> Self::Iterator {
        std::iter::once(self)
    }
}
