# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":cython_library.bzl", cython_library_spec = "cython_library")
load(":cython_static_extension.bzl", cython_static_extension_spec = "cython_static_extension")
load(":cython_toolchain.bzl", cython_toolchain_spec = "cython_toolchain")

specs = {
    "cython_library": cython_library_spec,
    "cython_static_extension": cython_static_extension_spec,
    "cython_toolchain": cython_toolchain_spec,
}

rules = make_rules(specs)

load_symbols(rules)
