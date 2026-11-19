/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Implementation of several server commands.

pub(crate) mod build;
pub(crate) mod complete;
pub(crate) mod debug_anon_targets;
pub(crate) mod debug_eval;
pub(crate) mod expand_external_cells;
pub(crate) mod explain;
#[cfg(fbcode_build)]
pub(crate) mod explain_code;
pub(crate) mod init_commands;
pub(crate) mod install;

pub fn init_late_bindings() {
    init_commands::init_other_server_commands();
}
