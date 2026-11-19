# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@shim//:shims.bzl",
    _proto_srcs = "proto_srcs",
    _rust_protobuf_library = "rust_protobuf_library",
)

rust_protobuf_library = _rust_protobuf_library
proto_srcs = _proto_srcs
