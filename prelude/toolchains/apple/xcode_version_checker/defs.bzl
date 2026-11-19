# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:native.bzl", _native = "native")

def xcode_command_alias(name, xcode_version = None, xcode_product_build = None, **kwargs):
    xcode_version_specified = xcode_version and len(xcode_version) > 0
    xcode_product_build_specified = xcode_product_build and len(xcode_product_build) > 0

    if not xcode_version_specified and not xcode_product_build_specified:
        fail("Must specify either Xcode version or Xcode product build")
    if xcode_version_specified and xcode_product_build_specified:
        fail("Must specify only one of Xcode version or Xcode product build but not both")

    version_args = ["-n", xcode_version] if xcode_version_specified else ["-b", xcode_product_build]
    original_args = kwargs.pop("args", [])
    all_args = version_args + original_args

    env = kwargs.pop("env", {})
    xcode_cache_seed = read_root_config("apple", "xcode_cache_seed", None)
    if xcode_cache_seed:
        # It's possible that Mac RE does not distinguish between Xcode versions,
        # in which case we can reset the caches by setting a unique env var.
        # Since the env var is part of the cache key, that means that any actions
        # depending on selected Xcode will get recomputed.
        env["BUCK2_XCODE_CACHE_SEED"] = xcode_cache_seed

    # This setup means that all `xcode_command_alias()` would effectively become:
    #   xcode_version_checker VERSION ORIGINAL_COMMAND ORIGINAL_COMMAND_ARGS
    # `xcode_version_checker` will check the currently selected Xcode and
    # if it matches, it will then execute `ORIGINAL_COMMAND`
    # passing `ORIGINAL_COMMAND_ARGS`.
    _native.command_alias(
        name = name, exe = "prelude//toolchains/apple/xcode_version_checker:xcode_version_checker", args = all_args, env = env, visibility = ["PUBLIC"], **kwargs
    )
