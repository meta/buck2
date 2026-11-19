/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;

/// CommandLineSink is the destination into which `CommandLineBuilder` writes the fully prepared
/// command line.
///
/// This is almost always just `Vec<String>`.
pub trait CommandLineSink {
    fn push_arg(&mut self, s: Cow<'_, str>);
}

impl CommandLineSink for Vec<String> {
    fn push_arg(&mut self, s: Cow<'_, str>) {
        self.push(s.into_owned())
    }
}

/// A command line is normally a list of strings; this type is to be used when you expect just one.
///
/// This should normally be used with a `CommandLineBuilder` on which you expect to immediately
/// call `push_scope_delimiter`.
pub enum SingletonCommandLineSink {
    None,
    Finished(String),
    Error(buck2_error::Error),
}

impl SingletonCommandLineSink {
    pub fn new() -> Self {
        Self::None
    }

    pub fn finalize(self) -> buck2_error::Result<String> {
        match self {
            Self::None => Err(buck2_error::internal_error!(
                "SingletonCommandLineSink not written",
            )),
            Self::Finished(s) => Ok(s),
            Self::Error(e) => Err(e),
        }
    }
}

impl CommandLineSink for SingletonCommandLineSink {
    fn push_arg(&mut self, s: Cow<'_, str>) {
        match self {
            Self::None => {
                *self = Self::Finished(s.into_owned());
            }
            Self::Finished(_) => {
                *self = Self::Error(buck2_error::internal_error!(
                    "SingletonCommandLineSink written more than once"
                ));
            }
            Self::Error(_) => {}
        }
    }
}
