# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import contextlib
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Iterator, Sequence

import make_py_package_inplace

TEMPLATE: Path = Path(__file__).resolve().parent.parent / "run_inplace.py.in"

# An interpreter that some other rule built, as buck2 spells it: relative to the
# project root, which is also the cwd the action runs in.
ARTIFACT_PYTHON = "buck-out/v2/gen/toolchains/cpython/bin/python3"

STUB_RUNNER = """\
def run_as_main(main_module, main_function):
    import runpy

    runpy.run_module(main_module, run_name="__main__", alter_sys=True)
"""

APP = """\
import os

print("ran with PYTHONPATH", os.environ["PYTHONPATH"])
"""


@contextlib.contextmanager
def _cwd(path: Path) -> Iterator[None]:
    old = os.getcwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(old)


def _make_bootstrapper(
    project_root: Path,
    python: str,
    output: str = "out/bin.pex",
    modules_dir: str = "out/bin#link-tree",
) -> Path:
    """Invoke the tool the way buck2 does: from the project root, with relative paths."""

    argv = [
        "make_py_package_inplace.py",
        "--template",
        str(TEMPLATE),
        "--python",
        python,
        "--host-python",
        sys.executable,
        "--entry-point",
        "app",
        "--main-runner",
        "stub_runner.run_as_main",
        "--modules-dir",
        modules_dir,
        output,
    ]

    old_argv = sys.argv
    sys.argv = argv
    try:
        with _cwd(project_root):
            make_py_package_inplace.main()
    finally:
        sys.argv = old_argv
    return project_root / output


class ShebangTest(unittest.TestCase):
    def test_bare_interpreter_is_left_to_env(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            pex = _make_bootstrapper(Path(tmpdir), "python3")
            self.assertEqual(
                pex.read_text(encoding="utf8").splitlines()[0],
                "#!/usr/bin/env python3",
            )

    def test_absolute_interpreter_is_used_verbatim(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            pex = _make_bootstrapper(Path(tmpdir), "/usr/bin/python3")
            self.assertEqual(
                pex.read_text(encoding="utf8").splitlines()[0],
                "#!/usr/bin/env /usr/bin/python3",
            )

    def test_artifact_interpreter_uses_sh_trampoline(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            pex = _make_bootstrapper(Path(tmpdir), ARTIFACT_PYTHON)
            lines = pex.read_text(encoding="utf8").splitlines()

            self.assertEqual(lines[0], "#!/bin/sh")
            self.assertIn(
                'exec "${_self%/*}/../buck-out/v2/gen/toolchains/cpython/bin/python3"'
                ' "$0" "$@"',
                lines,
            )

    def test_output_holds_no_absolute_path(self) -> None:
        # The whole point: an absolute path here is this machine's, and gets shipped to
        # everyone else that reads the action out of the cache.
        with tempfile.TemporaryDirectory() as tmpdir:
            pex = _make_bootstrapper(Path(tmpdir), ARTIFACT_PYTHON)
            self.assertNotIn(tmpdir, pex.read_text(encoding="utf8"))

    def test_output_does_not_depend_on_project_root(self) -> None:
        outputs = []
        for name in ("short", "a/much/deeper/checkout/path"):
            with tempfile.TemporaryDirectory() as tmpdir:
                root = Path(tmpdir) / name
                root.mkdir(parents=True)
                outputs.append(_make_bootstrapper(root, ARTIFACT_PYTHON).read_bytes())
        self.assertEqual(outputs[0], outputs[1])

    def test_bootstrapper_is_valid_python(self) -> None:
        # Windows ignores `#!` and runs the bootstrapper as `python <pex>`, so the shell
        # trampoline has to parse as Python too.
        with tempfile.TemporaryDirectory() as tmpdir:
            pex = _make_bootstrapper(Path(tmpdir), ARTIFACT_PYTHON)
            compile(pex.read_text(encoding="utf8"), str(pex), "exec")


@unittest.skipIf(sys.platform == "win32", "no #! support on Windows")
class TrampolineRuntimeTest(unittest.TestCase):
    def _make_project(self, tmpdir: str, name: str = "project") -> Path:
        root = Path(tmpdir) / name
        (root / "out/bin#link-tree").mkdir(parents=True)
        (root / "out/bin#link-tree/stub_runner.py").write_text(
            STUB_RUNNER, encoding="utf8"
        )
        (root / "out/bin#link-tree/app.py").write_text(APP, encoding="utf8")

        # Stand in for a hermetic interpreter built by some other rule.
        (root / "buck-out/interpreter").mkdir(parents=True)
        os.symlink(
            os.path.realpath(sys.executable), root / "buck-out/interpreter/python3"
        )

        _make_bootstrapper(root, "buck-out/interpreter/python3")
        return root

    def _run(self, argv: Sequence[str], cwd: str) -> str:
        proc = subprocess.run(
            list(argv),
            cwd=cwd,
            capture_output=True,
            encoding="utf8",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        return proc.stdout

    def test_runs_from_an_unrelated_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            root = self._make_project(tmpdir)
            out = self._run([str(root / "out/bin.pex")], cwd=tempfile.gettempdir())
            self.assertIn(
                "ran with PYTHONPATH {}".format(root / "out/bin#link-tree"), out
            )

    def test_runs_with_a_relative_argv0(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            root = self._make_project(tmpdir)
            out = self._run(["./out/bin.pex"], cwd=str(root))
            self.assertIn(
                "ran with PYTHONPATH {}".format(root / "out/bin#link-tree"), out
            )

    def test_runs_from_a_path_with_spaces(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            root = self._make_project(tmpdir, name="a project")
            out = self._run([str(root / "out/bin.pex")], cwd=tempfile.gettempdir())
            self.assertIn(
                "ran with PYTHONPATH {}".format(root / "out/bin#link-tree"), out
            )

    def test_runs_through_a_chain_of_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            root = self._make_project(tmpdir)
            # A relative link, then an absolute one pointing at it.
            os.symlink("out/bin.pex", root / "relative-link.pex")
            os.symlink(root / "relative-link.pex", Path(tmpdir) / "absolute-link.pex")
            out = self._run(
                [str(Path(tmpdir) / "absolute-link.pex")], cwd=tempfile.gettempdir()
            )
            self.assertIn(
                "ran with PYTHONPATH {}".format(root / "out/bin#link-tree"), out
            )

    def test_runs_without_a_usable_path(self) -> None:
        # Pexes get used as build tools, where buck2 scrubs the environment. Nothing in
        # the trampoline may depend on finding a helper on $PATH.
        with tempfile.TemporaryDirectory() as tmpdir:
            pex = self._make_project(tmpdir) / "out/bin.pex"
            proc = subprocess.run(
                [str(pex)],
                env={},
                capture_output=True,
                encoding="utf8",
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertIn("ran with PYTHONPATH", proc.stdout)


@unittest.skipIf(sys.version_info < (3, 11), "PYTHONSAFEPATH is 3.11+")
class SafePathTest(unittest.TestCase):
    """The startup code drops the cwd entry that `-c` adds to `sys.path`."""

    @staticmethod
    def _launch(tmpdir: str, env: dict[str, str]) -> "subprocess.CompletedProcess[str]":
        root = Path(tmpdir)
        tree = root / "out/bin#link-tree"
        tree.mkdir(parents=True)
        (tree / "stub_runner.py").write_text(STUB_RUNNER, encoding="utf8")
        (tree / "app.py").write_text(APP, encoding="utf8")
        pex = _make_bootstrapper(root, sys.executable)
        return subprocess.run(
            [str(pex)],
            capture_output=True,
            encoding="utf8",
            env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), **env},
        )

    def test_safe_path_fails_the_assert(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            proc = self._launch(tmpdir, {"PYTHONSAFEPATH": "1"})
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("AssertionError", proc.stderr)
