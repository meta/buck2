/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_settings_derive::buck_settings_section;

/// The HTTP client the daemon downloads with.
#[buck_settings_section(name = "http", version = 0, scope = daemon)]
pub struct Http {
    /// Milliseconds to establish a connection; `0` for no limit, unset for
    /// the client's default.
    #[setting(
        buckconfig_fallback = "http.connect_timeout_ms",
        overridable_in(CommandLine, LocalSettings)
    )]
    connect_timeout_ms: Option<u64>,
    /// Milliseconds to wait for response data; `0` for no limit, unset for
    /// the client's default.
    #[setting(
        buckconfig_fallback = "http.read_timeout_ms",
        overridable_in(CommandLine, LocalSettings)
    )]
    read_timeout_ms: Option<u64>,
    /// Milliseconds to send request data; `0` for no limit, unset for the
    /// client's default.
    #[setting(
        buckconfig_fallback = "http.write_timeout_ms",
        overridable_in(CommandLine, LocalSettings)
    )]
    write_timeout_ms: Option<u64>,
    /// Whether HTTP/2 is negotiated.
    #[setting(
        buckconfig_fallback = "http.http2",
        default = true,
        overridable_in(CommandLine, LocalSettings)
    )]
    http2: Option<bool>,
    /// Redirects followed per request; unset for the client's default.
    #[setting(
        buckconfig_fallback = "http.max_redirects",
        overridable_in(CommandLine, LocalSettings)
    )]
    max_redirects: Option<usize>,
    /// Requests in flight at once; unset for no limit.
    #[setting(
        buckconfig_fallback = "http.max_concurrent_requests",
        overridable_in(CommandLine, LocalSettings)
    )]
    max_concurrent_requests: Option<usize>,
}
