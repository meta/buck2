# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import os
import unittest
from unittest import mock

import parse_julia_cmd

SHARED_LIBS = "artifacts/lib/../__shared_libs_symlink_tree__"


def command_env(toolchain_env, inherited_env):
    json_data = {
        "env": toolchain_env,
        "julia_args": [],
        "julia_binary": "x/bin/julia",
        "julia_flags": [],
        "lib_path": "x/lib",
        "main": "x/main.jl",
    }
    with mock.patch.dict(os.environ, inherited_env, clear=True):
        _, env = parse_julia_cmd.build_command(
            json_data, "artifacts", "/libs", "/depot"
        )
    return env


class ParseJuliaCmdTest(unittest.TestCase):
    def test_an_inherited_library_path_is_kept_after_the_shared_libs(self):
        env = command_env(None, {"LD_LIBRARY_PATH": "/opt/lib"})
        self.assertEqual(env["LD_LIBRARY_PATH"], SHARED_LIBS + ":/opt/lib")

    def test_the_toolchain_env_is_passed_to_julia(self):
        env = command_env({"MY_TOOLCHAIN_VAR": "1"}, {"PATH": "/bin"})
        self.assertEqual(env["MY_TOOLCHAIN_VAR"], "1")
        self.assertEqual(env["PATH"], "/bin")

    def test_an_unset_library_path_leaves_no_empty_entry(self):
        env = command_env(None, {"PATH": "/bin"})
        self.assertEqual(env["LD_LIBRARY_PATH"], SHARED_LIBS)
