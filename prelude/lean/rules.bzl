# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Rules for Lean 4.

load("@prelude//lean:rules.bzl", "lean_library")
"""

load(":binary.bzl", _lean_binary = "lean_binary", _lean_test = "lean_test")
load(":library.bzl", _lean_library = "lean_library")
load(":providers.bzl", _LeanLibraryInfo = "LeanLibraryInfo", _LeanToolchainInfo = "LeanToolchainInfo")
load(":toolchain.bzl", _lean_toolchain = "lean_toolchain")

LeanLibraryInfo = _LeanLibraryInfo
LeanToolchainInfo = _LeanToolchainInfo
lean_binary = _lean_binary
lean_library = _lean_library
lean_test = _lean_test
lean_toolchain = _lean_toolchain
