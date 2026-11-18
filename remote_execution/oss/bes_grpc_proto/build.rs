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
        "proto/build_status.proto",
        "proto/build_events.proto",
        "proto/publish_build_event.proto",
    ];

    let google_proto_root = std::env::var("GOOGLE_PROTO_ROOT").ok();
    let google_proto_root = google_proto_root
        .as_deref()
        .map(|root| format!("{root}/proto"));
    let mut includes = vec!["./proto/", "../google_grpc_proto/proto/"];
    if let Some(include) = google_proto_root.as_deref() {
        includes.push(include);
    }

    let builder = buck2_protoc_dev::configure();
    unsafe { builder.setup_protoc() }
        .extern_path(".google.api", "::google_grpc_proto::google::api")
        .compile(proto_files, &includes)
}
