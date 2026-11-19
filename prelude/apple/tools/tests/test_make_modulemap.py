# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import io
import unittest

from apple.tools.make_modulemap import _write_submodules


class SubmodulesTest(unittest.TestCase):
    """With `--use-submodules` every header gets a submodule named after its file, and a name
    that is a modulemap keyword gets a `_` suffix."""

    def _modulemap(self, headers: list) -> str:
        f = io.StringIO()
        _write_submodules(f, "Foo", {"Foo/" + h: "Foo/" + h for h in headers}, False)
        return f.getvalue()

    def test_keyword_headers_get_distinct_names(self) -> None:
        modulemap = self._modulemap(
            ["module.h", "module_.h", "private.h", "private/impl.h"]
        )

        self.assertEqual(modulemap.count("module module_ {"), 1)
        self.assertEqual(modulemap.count("module module__ {"), 1)
        self.assertEqual(modulemap.count("module private_ {"), 1)
        self.assertEqual(modulemap.count("module private__ {"), 1)
