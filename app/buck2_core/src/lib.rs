/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(never_type)]
#![cfg_attr(test, feature(pattern))]
#![feature(impl_trait_in_assoc_type)]
#![feature(try_trait_v2)]
#![feature(try_trait_v2_residual)]

// Re-export these because we don't want to make people add a dependency on this crate everywhere
pub use buck2_env::env;
pub use buck2_env::soft_error as error;

mod ascii_char_set;
pub mod async_once_cell;
pub mod build_file_path;
pub mod bxl;
pub mod bzl;
pub mod bzl_global;
pub mod category;
pub mod cells;
pub mod ci;
pub mod client_only;
pub mod configuration;
pub mod content_hash;
pub mod deferred;
pub mod directory_digest;
pub mod event;
pub mod execution_types;
pub mod faster_directories;
pub mod fs;
pub mod global_cfg_options;
pub use buck2_fs::io_counters;
pub mod logging;
pub mod package;
pub mod pattern;
pub mod plugins;
pub mod provider;
pub mod quick_debug_event;
pub mod rollout_percentage;
pub mod target;
pub mod target_aliases;
pub mod unsafe_send_future;

// Re-export macros from buck2_env so they're available at the crate root
pub use buck2_env::env::buck2_env;
pub use buck2_env::env::buck2_env_name;
// Re-export these macros at the crate root so they work like before when error was #[macro_use]
#[doc(inline)]
pub use buck2_env::soft_error::deprecation;
#[doc(inline)]
pub use buck2_env::soft_error::deprecation_or_panic;
#[doc(inline)]
pub use buck2_env::soft_error::soft_error;
#[doc(inline)]
pub use buck2_env::soft_error::soft_error_or_panic;
#[doc(inline)]
pub use buck2_env::soft_error::tag_error;
#[doc(inline)]
pub use buck2_env::soft_error::tag_result;
#[doc(inline)]
pub use buck2_env::soft_error::warn_user;
#[doc(inline)]
pub use buck2_env::soft_error::warn_user_or_panic;

/// Marker for things that are only sensible to use inside Facebook,
/// not intended to be complete, but intended to be useful to audit
/// en-mass at some point in the future.
pub fn facebook_only() {}

/// Emit one expression or another depending on whether this is an open source or internal build.
#[macro_export]
macro_rules! if_else_opensource {
    ($opensource:expr, $internal:expr $(,)?
    ) => {
        // @oss-disable: $internal
        $opensource // @oss-enable
    };
}

#[inline]
pub fn is_open_source() -> bool {
    if_else_opensource!(true, false)
}
