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

from makefile_to_dep_file import rewrite_dep_file
from show_headers_to_dep_file import parse_into_dep_file
from show_includes_to_dep_file import rewrite_dep_file_for_msvc


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


class ShowIncludesTest(unittest.TestCase):
    """`/showIncludes` prints one `Note: including file:` line per included header on stdout,
    indented by the include depth."""

    def _deps(self, show_includes_output: str) -> list:
        with tempfile.TemporaryDirectory() as temp_dir:
            dep_file = os.path.join(temp_dir, "out.d")
            with contextlib.redirect_stderr(io.StringIO()):
                rewrite_dep_file_for_msvc(show_includes_output, dep_file, "a.cpp")
            with open(dep_file) as f:
                return f.read().splitlines()

    def test_cl_source_file_line_is_skipped(self) -> None:
        # cl.exe prints the name of the source file first.
        deps = self._deps(
            "a.cpp\nNote: including file: h1.h\nNote: including file:  h2.h\n"
        )

        self.assertEqual(deps, ["h1.h", "h2.h", "a.cpp"])

    def test_clang_cl_output_keeps_first_header(self) -> None:
        # What clang 21 prints for `clang --driver-mode=cl /c a.cpp /showIncludes` when
        # `a.cpp` includes `"h1.h"` and `"h2.h"`: no source file line.
        deps = self._deps(
            "Note: including file: ./h1.h\nNote: including file: ./h2.h\n"
        )

        self.assertEqual(deps, ["h1.h", "h2.h", "a.cpp"])


class MakefileTest(unittest.TestCase):
    """`-MD` writes a make rule; clang and gcc escape a space as `\\ `, `#` as `\\#` and
    `$` as `$$`."""

    def _deps(self, makefile: str) -> list:
        with tempfile.TemporaryDirectory() as temp_dir:
            src = os.path.join(temp_dir, "in.d")
            dep_file = os.path.join(temp_dir, "out.d")
            with open(src, "w") as f:
                f.write(makefile)
            rewrite_dep_file(src, dep_file)
            with open(dep_file) as f:
                return f.read().splitlines()

    def test_escaped_file_names(self) -> None:
        # What clang 21 writes for `clang -c m.c -o m.o -MD -MF m.d` when `m.c` includes
        # `"ha#sh.h"`, `"dol$lar.h"`, `"sp ace.h"`, `"pct%x.h"` and `"sub dir/n#s.h"`.
        deps = self._deps(
            "m.o: \\\n  m.c ha\\#sh.h dol$$lar.h sp\\ ace.h pct%x.h \\\n  sub\\ dir/n\\#s.h\n"
        )

        self.assertEqual(
            deps,
            ["m.c", "ha#sh.h", "dol$lar.h", "sp ace.h", "pct%x.h", "sub dir/n#s.h"],
        )

    def test_backslashes_before_escaped_characters(self) -> None:
        # Make reads 2N+1 backslashes before a space as N backslashes and a space,
        # and 2N backslashes as N backslashes and the end of the name. Before `#`,
        # only one backslash is added. Backslashes then become `/`.
        deps = self._deps("o: a\\\\\\ b.h c\\\\ d.h e\\\\#f.h\n")

        self.assertEqual(deps, ["a/ b.h", "c/", "d.h", "e/#f.h"])
