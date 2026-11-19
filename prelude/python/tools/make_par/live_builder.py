# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-ignore-all-errors

import os
import shutil

from par_builder import ParBuilder
from util import (
    get_user_main,
    make_clean_dir,
    make_ld_library_path,
    make_ld_preload,
    RUN_LIVEPAR_MAIN_MODULE,
    SOURCE_SUFFIX,
)


def _shell_assignment(entry):
    name, _, value = entry.partition("=")
    # Double-quoted as in the fastzip wrapper: `$VAR`, `${X:-}`, `$(...)` and backticks
    # still expand, while spaces and apostrophes no longer break the `export` line.
    # A `\` is escaped so that a value ending in one cannot escape the closing quote.
    value = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'{name}="{value}"'


class LiveBuilder(ParBuilder):
    def __init__(self, options, manifest, mode=0o755, linktree_suffix="#linktree"):
        # Default to "default" warnings, as per buck1/buck2.
        super().__init__(options, manifest, mode)
        # foo.par -> foo#linktree
        # foo.bar.par -> foo.bar#linktree
        if self.python_home:
            if not os.path.isabs(self.python_home):
                self.python_home = os.path.join("$DIR", self.python_home)
            self.runtime_env.append(f"PYTHONHOME={self.python_home}")

        if options.live_link_tree is not None:
            self.linktree = options.live_link_tree
        else:
            self.linktree = options.output.rsplit(".", 1)[0] + linktree_suffix
        self.copy_files = options.copy_files is True

    def _postbuild(self):
        pass

    def _gen_contents(self, output_file):
        manifest = self.manifest
        with open(
            os.path.join(
                os.path.dirname(__file__), RUN_LIVEPAR_MAIN_MODULE + SOURCE_SUFFIX
            )
        ) as f:
            manifest.add_autogen_module(RUN_LIVEPAR_MAIN_MODULE, f.read())

        linktree = self.linktree
        make_clean_dir(linktree)

        dirs = {linktree}
        for entry in manifest.entries:
            dest_path = os.path.join(linktree, entry.dest_path)
            dest_dir = os.path.dirname(dest_path)
            if dest_dir not in dirs:
                os.makedirs(dest_dir)
                while dest_dir not in dirs:
                    dirs.add(dest_dir)
                    dest_dir = os.path.dirname(dest_dir)

            if entry.src_data is None:
                assert entry.src_path is not None
                if self.copy_files:
                    shutil.copyfile(entry.src_path, dest_path)
                    os.chmod(dest_path, os.stat(entry.src_path).st_mode)
                else:
                    # Add a symlink to the source file
                    if entry.src_path.startswith("/"):
                        # if src_path is absolute, use it as link_path (no need to calculate relative path).
                        link_path = entry.src_path
                    else:
                        link_path = os.path.relpath(
                            entry.src_path, os.path.dirname(dest_path)
                        )
                    os.symlink(link_path, dest_path)
            else:
                # For auto-generated files, write them out as
                # regular files in the linktree.
                assert entry.src_path is None
                with open(dest_path, "w") as handle:
                    handle.write(entry.src_data)

        header = self._gen_header()
        output_file.write(bytes(header, "UTF-8"))

        bootstrap_template = os.path.join(
            os.path.dirname(__file__), "_lpar_bootstrap.sh.template"
        )
        with open(linktree + "/_bootstrap.sh", "w") as f:
            f.write(self._gen_interp_file(bootstrap_template))
            os.chmod(f.name, self.mode)

    def _gen_header(self):
        linktreedir = self.linktree.rsplit("/", 1)[-1]

        absolute_base_dir = os.path.abspath(self.linktree)
        if absolute_base_dir.startswith("/re_cwd"):
            # This is a remote execution job, the actual path will be different
            # when execute locally. Updating the absolute path accordingly.
            # This change assumes fbsource is always under $HOME, which is generally true for linux.
            absolute_base_dir = "${HOME}/fbsource/" + self.linktree
        script = (
            "#!/bin/bash\n"
            "# {argcomplete_hint}\n"
            "# LINKTREEDIR={linktreedir}\n"
            "\n"
            'export FB_LPAR_INVOKED_NAME="$0"\n'
            "# This environment variable is immediately unset on startup but will\n"
            "# also appear in e.g. `multiprocessing` workers, and so serves as an\n"
            "# audit trail back to the originating PAR (and can be read via e.g.\n"
            "# `/proc/<pid>/environ`)\n"
            'export PAR_INVOKED_NAME_TAG="$0"\n'
            "# Make live par compatible with the par_builder and export the same environment variable\n"
            "# See https://fburl.com/workplace/u561pr0v for more details\n"
            'export FB_XAR_INVOKED_NAME="$0"\n'
            '__lpar_dir="$(dirname "$(readlink -f "$0")")"\n'
            'if [ -f "$__lpar_dir/{linktreedir}/_bootstrap.sh" ]; then\n'
            '  MAIN_MODULE="$__lpar_dir/{linktreedir}/{run_livepar}"\n'
            '  exec "$__lpar_dir"/{linktreedir}/_bootstrap.sh "${{MAIN_MODULE}}" "$@"\n'
            "else\n"
            '  MAIN_MODULE="{absolute_base_dir}/{run_livepar}"\n'
            '  exec "{absolute_base_dir}/_bootstrap.sh" "${{MAIN_MODULE}}" "$@"\n'
            "fi\n"
        )
        return script.format(
            absolute_base_dir=absolute_base_dir,
            linktreedir=linktreedir,
            argcomplete_hint=self.options.argcomplete_hint,
            run_livepar=RUN_LIVEPAR_MAIN_MODULE + SOURCE_SUFFIX,
        )

    def _gen_interp_file(self, template, interp=None):
        options = self.options
        # We do not use readlink because the generated _bootstrap.sh may be a symlink
        base_dir = '$(dirname "$0")'
        ld_library_path = make_ld_library_path(
            options, options.target.lib_path_env, ["$BASE_DIR"]
        )
        main_module, main_function = get_user_main(options, self.manifest)
        py_cmd = self._get_python_command(base_dir="$BASE_DIR")
        cmd = interp or py_cmd

        if self.runtime_env:
            env = "export " + " ".join(_shell_assignment(e) for e in self.runtime_env)
        else:
            env = ""

        with open(template, "r") as f:
            script = f.read()

        lib_preload_env = options.target.lib_preload_env
        if options.ld_preload:
            ld_preload_list = make_ld_preload(
                options.target.lib_preload_env,
                options.ld_preload,
                base_dir="$BASE_DIR",
            )
            ld_preload = f"export {lib_preload_env}={ld_preload_list}\n"
        else:
            ld_preload = ""

        main_runner_module, main_runner_function = options.main_runner.rsplit(".", 1)

        return script.format(
            base_dir=base_dir,
            ld_library_path=ld_library_path,
            ld_preload=ld_preload,
            argcomplete_hint=options.argcomplete_hint,
            main_module=main_module,
            main_function=main_function,
            main_runner_module=main_runner_module,
            main_runner_function=main_runner_function,
            lib_path_env=options.target.lib_path_env,
            lib_preload_env=lib_preload_env,
            env=env,
            cmd=cmd,
        )
