/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use serde_json::error::Category;

use crate::conversion::stds::io::io_error_kind_to_error_tag;

impl From<serde_json::Error> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: serde_json::Error) -> Self {
        let error_tag = match value.classify() {
            Category::Data | Category::Syntax => crate::ErrorTag::Input,
            Category::Eof => crate::ErrorTag::SerdeJson,
            Category::Io => match value.io_error_kind() {
                Some(error_kind) => io_error_kind_to_error_tag(error_kind),
                None => crate::ErrorTag::SerdeJson,
            },
        };

        crate::conversion::from_any_with_tag(value, error_tag)
    }
}

/// Trait extension to convert `buck2_error::Result<T>` to any serde error type.
pub trait BuckErrorSerde<T> {
    /// Convert a `buck2_error::Result<T>` to `Result<T, E>` where `E: serde::de::Error`.
    fn serde_err<E: serde::de::Error>(self) -> Result<T, E>;
}

impl<T> BuckErrorSerde<T> for crate::Result<T> {
    fn serde_err<E: serde::de::Error>(self) -> Result<T, E> {
        self.map_err(|e| E::custom(e.to_string()))
    }
}
