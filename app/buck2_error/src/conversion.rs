/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Conversion impls for different error types to 'buck2_error::Error'

pub mod clap;
pub mod dice;
pub mod eden;
pub mod hex;
pub mod http;
pub mod hyper;
pub mod nix;
pub mod other;
pub mod pagable;
pub mod pem;
pub mod prost;
pub mod regex;
pub mod report;
pub mod rusqlite;
pub mod serde;
pub mod stds;
pub mod superconsole;
#[cfg(fbcode_build)]
pub mod thrift;
pub mod tokio;
pub mod toml;
pub mod tonic;
pub mod uuid;
pub mod watchman;

/// Helper function that can be explicitly called to convert `std::error::Error` into `buck2_error`.
///
/// Common types should have a proper From implemented in this file, but this function is useful for
/// one-off error types in the codebase
pub use crate::any::from_any_with_tag;
