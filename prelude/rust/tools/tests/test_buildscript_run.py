#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import io
import os
import pathlib
import shutil
import sys
import tempfile
import unittest
from unittest import mock

from buildscript_run import main


class BuildscriptRunTest(unittest.TestCase):
    """`cargo_buildscript` runs a crate's build script through `buildscript_run.py` and feeds the
    `cargo:` directives it prints to rustc as flags."""

    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)

    def _executable(self, name: str, script: str) -> pathlib.Path:
        path = self.tmp / name
        path.write_text("#!/bin/sh\n" + script)
        path.chmod(0o755)
        return path

    def rustc_flags_for(self, output_lines: list) -> list:
        """The rustc flags produced for a build script that prints `output_lines`."""
        return self.run_buildscript(
            "".join("printf '%s\\n' '" + line + "'\n" for line in output_lines)
        )[0]

    def run_buildscript(self, script: str) -> tuple:
        """Run the build script `script` (sh); return (rustc flags, stdout bytes)."""
        buildscript = self._executable("build-script-build", script)
        rustc = self._executable("rustc", "echo rustc 1.0.0\n")
        rustc_cfg = self.tmp / "rustc_cfg"
        rustc_cfg.write_text("unix\n")
        manifest_dir = self.tmp / "manifest"
        manifest_dir.mkdir()
        outfile = self.tmp / "rustc_flags"
        argv = [
            "buildscript_run.py",
            f"--buildscript={buildscript}",
            f"--rustc-cfg={rustc_cfg}",
            f"--manifest-dir={manifest_dir}",
            f"--create-cwd={self.tmp / 'cwd'}",
            f"--outfile={outfile}",
        ]
        env = {
            **os.environ,
            "OUT_DIR": str(self.tmp / "out"),
            "RUSTC": str(rustc),
            "TARGET": "x86_64-unknown-linux-gnu",
            "HOST": "x86_64-unknown-linux-gnu",
        }
        self.stdout = io.TextIOWrapper(io.BytesIO(), encoding="utf-8")
        with mock.patch.object(sys, "argv", argv), mock.patch.dict(
            os.environ, env
        ), mock.patch.object(sys, "stdout", self.stdout):
            main()
        self.stdout.flush()
        return outfile.read_text().splitlines(), self.stdout.buffer.getvalue()

    def test_a_directive_is_read_from_the_trimmed_line(self) -> None:
        self.assertEqual(
            self.rustc_flags_for(
                [
                    "cargo:rustc-env=TRAIL=v  ",
                    "cargo:rustc-cfg=feature_x ",
                    "  cargo:rustc-cfg=y",
                ]
            ),
            ["--env-set=TRAIL=v", "--cfg=feature_x", "--cfg=y"],
        )

    def test_output_that_is_not_utf8(self) -> None:
        # cc-rs forwards the C compiler's output byte for byte as `cargo:warning=`
        # lines; with a Latin-1 source file or locale that is not UTF-8. Cargo
        # ignores such lines as directives and builds the crate.
        flags, stdout = self.run_buildscript(
            "printf 'cargo:warning=caf\\351\\n'\n"
            "printf 'cargo:rustc-cfg=bad\\351\\n'\n"
            "printf 'cargo:rustc-cfg=ok\\n'\n"
        )
        self.assertEqual(flags, ["--cfg=ok"])
        self.assertIn("cargo:warning=caf�\n".encode(), stdout)

    def test_stdout_is_shown_when_the_build_script_fails(self) -> None:
        # cc-rs prints the failing compiler command and its errors on stdout and
        # then panics; Cargo shows that stdout.
        with self.assertRaises(SystemExit) as exit:
            self.run_buildscript(
                "printf 'running: cc -c foo.c\\n'\n"
                "printf 'cargo:warning=foo.c:1:10: fatal error: missing.h: No such file\\n'\n"
                "printf 'caf\\351\\n'\n"
                "echo 'build script panicked' >&2\n"
                "exit 101\n"
            )
        self.assertEqual(exit.exception.code, 101)
        self.stdout.flush()
        self.assertEqual(
            self.stdout.buffer.getvalue(),
            b"running: cc -c foo.c\n"
            b"cargo:warning=foo.c:1:10: fatal error: missing.h: No such file\n"
            b"caf\xe9\n",
        )
