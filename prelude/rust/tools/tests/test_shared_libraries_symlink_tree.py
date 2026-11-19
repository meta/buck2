#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import json
import os
import pathlib
import shutil
import sys
import tempfile
import unittest
from unittest import mock

from shared_libraries_symlink_tree import main


class DwpSymlinkTreeTest(unittest.TestCase):
    """`run_action_shlib_symlink_tree` in rust/link_info.bzl lays out a binary's shared libraries
    and their `.dwp` files with this tool."""

    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)

    def dwp_tree_for(self, sonames: list) -> list:
        """The entries of the dwp tree for libraries named `sonames`, each with a dwp file."""
        entries = []
        for soname in sonames:
            so = self.tmp / soname
            so.write_text("so")
            dwp = self.tmp / (soname + ".dwp")
            dwp.write_text("dwp")
            entries.append([True, soname, str(so), str(dwp)])
        info = self.tmp / "info.json"
        info.write_text(json.dumps([entries]))
        argv = [
            "shared_libraries_symlink_tree.py",
            f"--shared_libs_symlink_tree={self.tmp / 'shlibs'}",
            f"--dwp_symlink_tree={self.tmp / 'dwp'}",
            f"--shared_libraries_info_json={info}",
        ]
        with mock.patch.object(sys, "argv", argv):
            main()
        return sorted(os.listdir(self.tmp / "dwp"))

    def test_a_dwp_link_keeps_the_whole_soname(self) -> None:
        self.assertEqual(self.dwp_tree_for(["libanyhow.so"]), ["libanyhow.so.dwp"])

    def test_versioned_sonames_get_their_own_links(self) -> None:
        self.assertEqual(
            self.dwp_tree_for(["libfoo.so.1", "libfoo.so.2"]),
            ["libfoo.so.1.dwp", "libfoo.so.2.dwp"],
        )
