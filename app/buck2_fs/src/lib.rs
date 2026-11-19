/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(impl_trait_in_assoc_type)]
#![feature(decl_macro)]

//! Project-agnostic filesystem utilities for Buck2.
//!
//! This crate contains filesystem types and utilities that have no knowledge
//! of Buck2 projects, cells, or build artifacts. It provides:
//! - Path types (ForwardRelativePath, AbsPath, AbsNormPath, FileName)
//! - Filesystem utilities (fs_util, async_fs_util)
//! - Working directory management (cwd, working_dir)
//!
//! For project-aware filesystem functionality, see the `buck2_core::fs` module.

pub mod async_fs_util;
pub mod cwd;
pub mod error;
pub mod fs_util;
pub mod io_counters;
pub mod paths;
pub mod working_dir;

pub use error::IoResultExt;
