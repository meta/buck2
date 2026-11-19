# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":cxx_lua_extension.bzl", cxx_lua_extension_spec = "cxx_lua_extension")
load(":lua_binary.bzl", lua_binary_spec = "lua_binary")
load(":lua_library.bzl", lua_library_spec = "lua_library")

specs = {
    "cxx_lua_extension": cxx_lua_extension_spec,
    "lua_binary": lua_binary_spec,
    "lua_library": lua_library_spec,
}

rules = make_rules(specs)

load_symbols(rules)
