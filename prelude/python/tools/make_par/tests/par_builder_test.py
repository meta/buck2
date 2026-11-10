# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import shlex
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from par_builder import ParBuilder


def _builder(output: str, python: str) -> ParBuilder:
    options = SimpleNamespace(
        output=output,
        strict_tabs=True,
        warnings=None,
        runtime_env=[],
        fix_os_argv=False,
        interpreter_flags=[],
        ld_preload=[],
        python=python,
        python_home=None,
        runtime_binary=None,
    )
    return ParBuilder(options, manifest=None)


class PythonCommandTest(unittest.TestCase):
    def test_path_like_interpreter_is_resolved_from_par(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            output = Path("buck-out/v2/art/fbsource/cfg/tools/copytree.par")
            python = Path("buck-out/v2/art/fbcode/cfg/python/trampoline")
            par = root / output
            interpreter = root / python
            par.parent.mkdir(parents=True)
            interpreter.parent.mkdir(parents=True)
            par.touch()
            interpreter.write_text(
                "#!/bin/sh\nprintf '%s\\n' \"$@\"\n", encoding="utf8"
            )
            interpreter.chmod(0o755)

            command = _builder(str(output), str(python))._get_python_command(
                resolve_python_from_output=True
            )
            unrelated = root / "unrelated" / "working" / "directory"
            unrelated.mkdir(parents=True)
            script = (
                'readlink_e() { readlink -e "$1"; }; '
                f"export FB_PAR_FILENAME={shlex.quote(str(par))}; "
                f"cd {shlex.quote(str(unrelated))}; {command}"
            )
            proc = subprocess.run(
                ["bash", "-c", script], capture_output=True, encoding="utf8"
            )

            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertEqual(proc.stdout, "-tt\n")

    def test_absolute_interpreter_is_unchanged(self) -> None:
        command = _builder(
            "out/tool.par", "/usr/local/bin/python3"
        )._get_python_command(resolve_python_from_output=True)
        self.assertEqual(command, "/usr/local/bin/python3 -tt")

    def test_path_lookup_interpreter_is_unchanged(self) -> None:
        command = _builder("out/tool.par", "python3")._get_python_command(
            resolve_python_from_output=True
        )
        self.assertEqual(command, "env python3 -tt")
