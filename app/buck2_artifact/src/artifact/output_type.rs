/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;

/// Is an output a file or a directory
#[derive(
    PartialEq,
    Eq,
    Hash,
    Debug,
    Copy,
    Clone,
    Dupe,
    Allocative,
    Ord,
    PartialOrd,
    strong_hash::StrongHash,
    Pagable
)]
pub enum OutputType {
    /// We don't know - used to represent legacy code that doesn't yet declare the output type properly.
    /// We aim to mostly remove this alternative over time.
    FileOrDirectory,
    File,
    Directory,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum OutputTypeError {
    #[error("Expected {1:?}, but `{0}` is already declared as {2:?}")]
    CheckPath(String, OutputType, OutputType),
}

impl OutputType {
    /// We are hoping for something of output type self, and got a path of an existing declared type.
    /// Raise an error if they are incompatible.
    pub fn check_path(
        self,
        path_for_error_message: impl Display,
        output_type: OutputType,
    ) -> buck2_error::Result<()> {
        // FIXME(JakobDegen): A path declared `FileOrDirectory` is treated as a file, so asking for
        // it as a directory should fail.
        if self == output_type
            || self == OutputType::FileOrDirectory
            || output_type == OutputType::FileOrDirectory
        {
            Ok(())
        } else {
            Err(
                OutputTypeError::CheckPath(path_for_error_message.to_string(), self, output_type)
                    .into(),
            )
        }
    }
}
