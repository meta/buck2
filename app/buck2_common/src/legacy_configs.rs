/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Contains utilities for dealing with buckv1 concepts (ex. buckv1's
//! .buckconfig files as configuration)

mod access;
pub use access::parse_buckconfig_metadata;
pub mod agent_context;
mod aggregator;
pub mod args;
pub mod cells;
pub mod configs;
pub mod dice;
pub mod file_ops;
pub mod key;
mod parser;
pub(crate) mod path;
pub mod view;
