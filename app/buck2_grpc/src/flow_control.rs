/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use tonic::transport::Endpoint;
use tonic::transport::Server;

/// HTTP/2 connection-level flow-control window, in bytes, for every gRPC connection buck2 opens
/// between its own processes: client to daemon, daemon to forkserver, and daemon to test executor.
///
/// Besides bounding how much unconsumed payload a peer may have in flight, this sizes h2's guard
/// against floods of small DATA frames. Since h2 0.4.16, each received DATA frame under 256 bytes
/// that the application has not yet read is charged against a budget of half the connection
/// window, and exhausting the budget closes the connection with `ENHANCE_YOUR_CALM`. buck2's
/// connections legitimately carry bursts of tiny messages (process exit events, per-test results,
/// console events) between trusted peers on one host, and hyper's default windows (5 MiB for
/// clients, 1 MiB for servers) were exhausted in production whenever the receiving side fell
/// behind. h2 does not yet expose the budget directly through hyper or tonic, so the window is
/// the knob.
pub const CONNECTION_WINDOW_SIZE: u32 = 64 * 1024 * 1024;

/// Applies buck2's flow-control settings to a client endpoint. Every `Endpoint` buck2 connects
/// with should go through here.
pub fn configure_endpoint(endpoint: Endpoint) -> Endpoint {
    endpoint.initial_connection_window_size(CONNECTION_WINDOW_SIZE)
}

/// A `Server::builder()` with buck2's flow-control settings applied. Every tonic server buck2
/// runs should be built from here.
pub fn server_builder() -> Server {
    Server::builder().initial_connection_window_size(CONNECTION_WINDOW_SIZE)
}
