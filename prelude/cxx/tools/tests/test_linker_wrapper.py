# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import contextlib
import io
import ntpath
import os
import tempfile
import unittest
from unittest import mock

import linker_wrapper
from linker_wrapper import rewrite_args

WORKING_DIR = "C:\\repo"
LIB = "buck-out\\v2\\gen\\root\\lib\\__foo__\\foo.lib"
ABS_LIB = "C:\\repo\\" + LIB


class RewriteArgsTest(unittest.TestCase):
    """The wrapper runs only on Windows, so path handling is checked with `ntpath`."""

    def _rewrite(self, arguments: list) -> list:
        with mock.patch.object(os, "path", ntpath):
            return rewrite_args(arguments, WORKING_DIR)

    def test_whole_archive_with_path_as_emitted_by_get_link_whole_args(self) -> None:
        # prelude/cxx/linker.bzl writes `/WHOLEARCHIVE:<path>` for every link_whole
        # archive on LinkerType("windows"), without a preceding plain library path.
        self.assertEqual(
            self._rewrite(['"/WHOLEARCHIVE:' + LIB + '"']),
            ["/WHOLEARCHIVE:" + ABS_LIB],
        )

    def test_whole_archive_with_basename_of_earlier_library(self) -> None:
        self.assertEqual(
            self._rewrite([LIB, "/WHOLEARCHIVE:foo.lib"]),
            [ABS_LIB, "/WHOLEARCHIVE:" + ABS_LIB],
        )

    def test_whole_archive_with_unknown_basename_is_kept(self) -> None:
        self.assertEqual(
            self._rewrite(["/WHOLEARCHIVE:kernel32.lib"]),
            ["/WHOLEARCHIVE:kernel32.lib"],
        )

    def test_bare_whole_archive_is_kept(self) -> None:
        self.assertEqual(self._rewrite(["/WHOLEARCHIVE"]), ["/WHOLEARCHIVE"])

    def test_whole_archive_option_is_not_recorded_as_a_library(self) -> None:
        # A later basename reference resolves to the library path, not to the
        # earlier `/WHOLEARCHIVE:...` option string.
        self.assertEqual(
            self._rewrite(["/WHOLEARCHIVE:" + LIB, "/WHOLEARCHIVE:foo.lib"]),
            ["/WHOLEARCHIVE:" + ABS_LIB, "/WHOLEARCHIVE:" + ABS_LIB],
        )

    def test_other_arguments_unchanged(self) -> None:
        self.assertEqual(
            self._rewrite(["/OUT:app.exe", "buck-out\\v2\\main.obj", "/DEBUG"]),
            ["/OUT:app.exe", "C:\\repo\\buck-out\\v2\\main.obj", "/DEBUG"],
        )


class MainTest(unittest.TestCase):
    """Runs the wrapper as the MSVC toolchain does: `linker_wrapper <link.exe> @argsfile`."""

    def test_link_whole_archive_from_argsfile(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            argsfile = os.path.join(temp_dir, "link.argsfile")
            with open(argsfile, "w") as f:
                f.write('"/OUT:app.exe"\n"/WHOLEARCHIVE:' + LIB + '"\n')

            linked = []

            class ResponseFile(io.BytesIO):
                name = "response.args"

                def __init__(self, mode, delete):
                    super().__init__()

                def __exit__(self, *exc_info):
                    linked.extend(self.getvalue().decode("utf-16").splitlines())

            argv = ["linker_wrapper", "link.exe", "@" + argsfile]
            with contextlib.ExitStack() as stack:
                stack.enter_context(mock.patch.object(os, "path", ntpath))
                stack.enter_context(
                    mock.patch.object(os, "getcwd", return_value=WORKING_DIR)
                )
                stack.enter_context(
                    mock.patch.object(
                        linker_wrapper.tempfile,
                        "NamedTemporaryFile",
                        side_effect=ResponseFile,
                    )
                )
                run = stack.enter_context(
                    mock.patch.object(linker_wrapper.subprocess, "run")
                )
                stack.enter_context(mock.patch.object(linker_wrapper.sys, "argv", argv))
                stack.enter_context(mock.patch.object(linker_wrapper.sys, "stderr"))
                linker_wrapper.main()

        run.assert_called_once_with(["link.exe", "@response.args"], check=True)
        self.assertEqual(linked, ['"/OUT:app.exe"', '"/WHOLEARCHIVE:' + ABS_LIB + '"'])


if __name__ == "__main__":
    unittest.main()
