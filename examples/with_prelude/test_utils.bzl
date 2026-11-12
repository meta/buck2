# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def assert_output(name, command, output):
    return native.genrule(
        name = name,
        bash = command + ' | grep "' + output + '" && touch "$OUT"',
        cmd_exe = command + ' | findstr "' + output + '" && type nul > "$OUT"',
        out = "out.txt",
    )

def haskell_library(deps = [], **kwargs):
    native.haskell_library(deps = deps + ["//third-party/haskell:base"], **kwargs)

def haskell_binary(deps = [], **kwargs):
    native.haskell_binary(
        deps = deps + ["//third-party/haskell:base"],
        **kwargs,
    )
