# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import os
import runpy
import sys
import tempfile
import unittest
from unittest import mock

# The tool is run as a script, as the rules invoke it.
SCRIPT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "script_template_processor.py",
)


def render_flags(compiler_flags):
    """Runs the tool on a one-macro template and returns the script it wrote."""
    with tempfile.TemporaryDirectory() as tmp_dir:
        template = os.path.join(tmp_dir, "ghci.sh")
        output = os.path.join(tmp_dir, "out.sh")
        with open(template, "w") as template_file:
            template_file.write("flags: <compiler_flags>\n")
        argv = [
            "script_template_processor.py",
            "--script_template",
            template,
            "--output",
            output,
            "--compiler_flags=" + compiler_flags,
            "--exposed_packages",
            "base",
        ]
        with mock.patch.object(sys, "argv", argv):
            try:
                runpy.run_path(SCRIPT, run_name="__main__")
            except SystemExit as exit_request:
                if exit_request.code:
                    raise
        with open(output) as output_file:
            return output_file.read()


class ScriptTemplateProcessorTest(unittest.TestCase):
    def test_an_escaped_quote_survives(self):
        self.assertEqual(
            render_flags('-optc-DFOO=\\"bar\\"'), 'flags: -optc-DFOO=\\"bar\\"\n'
        )

    def test_backslashes_in_a_value_are_kept(self):
        self.assertEqual(render_flags("-optP-DSEP=\\n"), "flags: -optP-DSEP=\\n\n")
        self.assertEqual(render_flags("-DWIN=C:\\tmp"), "flags: -DWIN=C:\\tmp\n")
        self.assertEqual(render_flags("-optP-DSEP=\\\\n"), "flags: -optP-DSEP=\\\\n\n")
        self.assertEqual(render_flags("-optP-DX=\\d"), "flags: -optP-DX=\\d\n")
