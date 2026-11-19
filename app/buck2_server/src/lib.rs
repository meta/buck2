/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(try_blocks)]

pub mod active_commands;
mod agent_config_guard;
mod agent_context_validation;
mod agent_host_guard;
mod clean_stale;
pub mod command_observer;
mod cpu_usage_collector;
mod ctx;
pub mod daemon;
mod dice_tracker;
mod file_status;
mod heartbeat_guard;
mod host_info;
mod hydration;
mod jemalloc_stats;
mod net_io;
pub(crate) mod new_generic;
mod paging;
pub mod profile;
pub(crate) mod profile_patterns;
pub mod profiling_manager;
mod snapshot;
mod subscription;
mod trace_io;
mod version_control_revision;
