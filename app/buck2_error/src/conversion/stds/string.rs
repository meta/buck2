/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::conversion::from_any_with_tag;

impl From<std::string::FromUtf8Error> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: std::string::FromUtf8Error) -> Self {
        from_any_with_tag(value, crate::ErrorTag::StringUtf8)
    }
}

impl From<std::str::Utf8Error> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: std::str::Utf8Error) -> Self {
        from_any_with_tag(value, crate::ErrorTag::StringUtf8)
    }
}

impl From<std::str::ParseBoolError> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: std::str::ParseBoolError) -> Self {
        from_any_with_tag(value, crate::ErrorTag::ParseBool)
    }
}
