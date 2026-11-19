# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import contextlib
import io
import os
import tempfile
import unittest

from show_headers_to_dep_file import parse_into_dep_file


class ShowHeadersTest(unittest.TestCase):
    """`clang -H` prints one line per included header: a run of dots for the include depth,
    one space, then the path."""

    def _deps(self, show_headers_output: str) -> list:
        with tempfile.TemporaryDirectory() as temp_dir:
            dep_file = os.path.join(temp_dir, "out.d")
            with contextlib.redirect_stderr(io.StringIO()):
                parse_into_dep_file(show_headers_output, dep_file, "r.c", 0)
            with open(dep_file) as f:
                return f.read().splitlines()

    def test_header_paths_are_kept_verbatim(self) -> None:
        # What clang 21 prints for `clang -H -I.gen -c ./r.c` when `r.c` includes `"v.h"`,
        # `<g.h>` and `"sub/my dir/s.h"`, and `g.h` includes `"h.h"` next to it.
        deps = self._deps(". ./v.h\n. .gen/g.h\n.. .gen/h.h\n. ./sub/my dir/s.h\n")

        self.assertEqual(deps, ["v.h", ".gen/g.h", ".gen/h.h", "sub/my dir/s.h", "r.c"])
