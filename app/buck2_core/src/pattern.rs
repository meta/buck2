/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Implements target pattern resolution.
#![doc = include_str!("pattern/target_pattern.md")]

mod ascii_pattern;
pub mod package;
pub mod parse_package;
#[allow(clippy::module_inception)]
pub mod pattern;
pub mod pattern_type;
pub mod query_file_literal;
pub mod unparsed;
