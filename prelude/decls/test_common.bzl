# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:toolchains_common.bzl", "toolchains_common")

def _attributes() -> dict[str, Attr]:
    return {
        "network_access": attrs.option(attrs.enum(["all", "none", "loopback", "strict", "private"]), default = None),
        "_test_toolchain": toolchains_common.test_toolchain(),
    }

test_common = struct(
    attributes = _attributes,
)
