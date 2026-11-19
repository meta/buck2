/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

mod channel;
mod flow_control;
mod server;
mod util;

pub use channel::DuplexChannel;
pub use channel::make_channel;
pub use flow_control::CONNECTION_WINDOW_SIZE;
pub use flow_control::configure_endpoint;
pub use flow_control::server_builder;
pub use server::ServerHandle;
pub use server::spawn_oneshot;
pub use util::to_tonic;
