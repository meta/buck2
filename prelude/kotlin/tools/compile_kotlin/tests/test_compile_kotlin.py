#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import pathlib
import tempfile
import unittest
import zipfile

from compile_kotlin import _zip_recursive


class _ReverseListingPath(type(pathlib.Path())):
    """A directory whose listing comes back in reverse name order, as a file system may."""

    def glob(self, pattern):
        return reversed(sorted(super().glob(pattern)))


class ZipRecursiveTest(unittest.TestCase):
    def _zip_generated_sources(self, temp_dir: str) -> list:
        root = pathlib.Path(temp_dir)
        package_dir = root / "generated" / "com" / "example"
        package_dir.mkdir(parents=True)
        for name in ["First.java", "Second.java"]:
            (package_dir / name).write_text("")
        archive = root / "generated.src.zip"
        _zip_recursive(archive, _ReverseListingPath(root / "generated"))
        with zipfile.ZipFile(archive) as z:
            return [
                name.rsplit("/", 1)[-1]
                for name in z.namelist()
                if name.endswith(".java")
            ]

    def test_archive_order_is_independent_of_the_directory_listing(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            self.assertEqual(
                self._zip_generated_sources(temp_dir), ["First.java", "Second.java"]
            )
