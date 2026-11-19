/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Implementation of test running.

pub mod command;
pub mod downward_api;
pub mod executor_launcher;
pub(crate) mod internal_runner;
pub(crate) mod local_resource_api;
pub mod local_resource_registry;
pub(crate) mod local_resource_setup;
pub mod orchestrator;
pub(crate) mod remote_storage;
pub mod session;
pub(crate) mod tcp;
pub mod translations;
#[cfg(unix)]
pub(crate) mod unix;

pub fn init_late_bindings() {
    command::init_test_command();
}
