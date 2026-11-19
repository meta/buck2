/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The file watcher tests that `tests/core/io` runs against each file watcher and file system, and
//! what they share.

pub mod cmd;
pub mod file_watcher;
pub mod file_watcher_dir_tests;
pub mod file_watcher_file_tests;
pub mod file_watcher_scm_tests;
pub mod file_watcher_symlink_tests;
pub mod file_watcher_tests;
pub mod utils;
