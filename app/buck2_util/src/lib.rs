/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(decl_macro)]
#![cfg_attr(not(windows), feature(linkage))]
#![feature(once_cell_try)]

pub mod arc_str;
pub mod async_move_clone;
pub mod commas;
pub mod early_command_timing;
pub mod env_vars;
pub mod exact_size_iter;
pub mod future;
pub mod golden_test_helper;
pub mod indent;
pub mod late_binding;
pub mod network_speed_average;
pub mod os;
pub mod per_thread_instruction_counter;
pub mod pgo;
pub mod pointer_identity;
pub mod process;
pub mod process_stats;
pub mod properly_reaped_child;
pub mod rtabort;
pub mod self_ref;
pub mod sliding_window;
pub mod strong_hasher;
pub mod system_stats;
pub mod threads;
pub mod time_span;
pub mod tokio_runtime;
pub mod truncate;
pub mod vec2;

// Re-export this to encourage people to use it in a fully qualified way.
pub use async_move_clone::async_move_clone;
// Re-export because not everything has `mini_vec` deps so this is a bit nicer.
pub use mini_vec::size_assert;
