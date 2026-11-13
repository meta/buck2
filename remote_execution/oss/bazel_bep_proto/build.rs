/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;

fn main() -> io::Result<()> {
    let proto_files = &[
        "proto/action_cache.proto",
        "proto/build_event_stream.proto",
        "proto/command_line.proto",
        "proto/failure_details.proto",
        "proto/invocation_policy.proto",
        "proto/option_filters.proto",
        "proto/package_load_metrics.proto",
        "proto/strategy_policy.proto",
    ];

    let builder = buck2_protoc_dev::configure();
    unsafe { builder.setup_protoc() }.compile(proto_files, &["."])
}
