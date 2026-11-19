# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-ignore-all-errors

import os
import tempfile


class ParBuilder:
    """
    Abstract class that represents the process of building a .par file
    """

    def __init__(self, options, manifest, mode=None):
        self.options = options
        self.manifest = manifest
        self.output = options.output
        self.strict_tabs = options.strict_tabs
        self.warnings = options.warnings
        self.mode = mode
        self.runtime_env = list(options.runtime_env)
        self.runtime_env.append(
            "FB_PAR_FIX_OS_ARGV={}".format("1" if options.fix_os_argv else "")
        )
        self.interpreter_flags = options.interpreter_flags
        self.ld_preload = options.ld_preload

        self.python = options.python
        self.python_home = options.python_home or None
        self.entry_point = options.runtime_binary or None

    def build(self):
        self._gen_file()
        self._postbuild()

    def _gen_file(self):
        (dirname, basename) = os.path.split(self.output)
        prefix = basename + ".tmp."
        with tempfile.TemporaryDirectory(dir=dirname, prefix=prefix) as tempdir:
            output_filename = os.path.join(tempdir, "output")
            with open(output_filename, "w+b") as output_file:
                # Call the function to generate the file contents
                self._gen_contents(output_file)

            # Rename the temporary output file to the real destination
            os.rename(output_filename, self.output)
            if self.mode:
                os.chmod(self.output, self.mode)

    def _gen_contents(self, output_file):
        raise NotImplementedError

    def _postbuild(self):
        raise NotImplementedError

    def _resolve_python_from_output(self, python):
        relative_python = os.path.relpath(python, os.path.dirname(self.output) or ".")
        return '"$(dirname "$(readlink_e "$FB_PAR_FILENAME")")/' + relative_python + '"'

    def _get_python_command(
        self,
        shebang=False,
        interp=None,
        base_dir=None,
        resolve_python_from_output=False,
    ):
        if self.entry_point:
            if not os.path.isabs(self.entry_point) and base_dir:
                cmd = [f"{base_dir}/{self.entry_point}"]
            else:
                cmd = [self.entry_point]
        elif interp:
            cmd = [interp]
        else:
            python = self.python
            if shebang and not os.path.isabs(python):
                # If shebang is true, the command will be used as the #! line in a
                # script.  Unfortunately, the #! line can only parse at most 2
                # arguments, so if we need to use env we can't use any other
                # arguments.  Therefore we ignore strict_tabs in this case.
                #
                # We also need to specify an absolute path to env in this case.
                return "/usr/bin/env " + python

            resolved_from_output = (
                resolve_python_from_output
                and not os.path.isabs(python)
                and bool(os.path.dirname(python))
            )
            if resolved_from_output:
                python = self._resolve_python_from_output(python)

            cmd = [python]
            if not os.path.isabs(python) and not resolved_from_output:
                cmd.insert(0, "env")

        flags = []
        if self.strict_tabs:
            flags.append("tt")
        if self.warnings is not None:
            flags.append(f"W{self.warnings}")
        if flags:
            cmd.append("-" + "".join(flags))

        if self.interpreter_flags:
            cmd.extend(self.interpreter_flags)

        return " ".join(cmd)
