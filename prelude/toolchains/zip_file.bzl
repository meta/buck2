# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//zip_file:zip_file_toolchain.bzl", "ZipFileToolchainInfo")

def zip_file_toolchain(name, **kwargs):
    kwargs["create_zip"] = "prelude//toolchains/android/src/com/facebook/buck/features/zip/rules/utils:zip_binary"

    _zip_file_toolchain_rule(name = name, **kwargs)

def _zip_file_toolchain_rule_impl(ctx):
    return [
        DefaultInfo(),
        ZipFileToolchainInfo(
            create_zip = ctx.attrs.create_zip[RunInfo],
        ),
    ]

_zip_file_toolchain_rule = rule(
    attrs = {
        "create_zip": attrs.dep(providers = [RunInfo]),
    },
    impl = _zip_file_toolchain_rule_impl,
    is_toolchain_rule = True,
)
