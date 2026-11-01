/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Common core components of buck2

#![feature(map_try_insert)]

pub mod argv;
pub mod buckd_connection;
pub mod build_count;
pub mod buildfiles;
pub mod cancellation;
pub mod cas_digest;
pub mod chunk_reader;
pub mod client_utils;
pub mod convert;
pub mod daemon_dir;
pub mod dice;
pub mod events;
pub mod external_cells;
pub mod external_symlink;
pub mod fbinit;
pub mod file_ops;
pub mod find_buildfile;
pub mod home_buck_tmp;
pub mod http;
pub mod ignores;
pub mod init;
pub mod invocation_paths;
pub mod invocation_paths_result;
pub mod invocation_roots;
pub mod io;
pub mod kill_util;
pub mod legacy_configs;
pub mod liveliness_observer;
pub mod local_resource_state;
pub mod manifold;
pub mod memory;
pub mod package_listing;
pub mod pagable;
pub mod pattern;
pub mod rlimits;
pub mod scope;
pub mod self_test_timeout;
pub mod sqlite;
pub mod starlark_profiler;
pub mod target_aliases;
pub mod temp_path;
pub mod tenant;
pub mod tenting;
