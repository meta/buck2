/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Starlark formatter library.
//!
//! This library provides programmatic access to the starlark_fmt formatting
//! functionality. It is used by the starlark_fmt binary and can be used by
//! other Rust projects like autodeps2.

mod api;
mod autofixes;
mod config;
mod formatting;
mod sort_key;
// Uses the Meta-internal `quickcheck_arbitrary_derive`, so it is compiled only
// in the internal (fbcode) build, not the open-source build.
#[cfg(all(test, fbcode_build))]
mod fuzz_starlark;

pub use api::FormattedSource;
pub use api::format_source;
pub use config::Config;
