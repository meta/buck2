#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


"""
Create a bootstrapper pex for inplace python binaries

This script:
    - Writes out a bootstrapper pex script that knows where this symlink tree is,
      and uses it, along with the provided entry point to run the python script.
      It does this by replacing a few special strings like <MODULES_DIR> and
      <MAIN_MODULE>

A full usage might be something like this:

$ cat template.in
(see prelude/python/run_inplace.py.in)
$ ./make_py_package_inplace.py  \\
    --template prelude/python/run_inplace.py.in \\
    # These two args create the hashbang for the bootstrapper script \\
    --python="/usr/bin/python3" \\
    --python-interpreter-flags="-Xgil=0" \\
    # This is based on the path in dests. This is the module that gets executed \\
    # to start program execution \\
    --entry-point=lib.foo  \\
    --output=bin.pex \\
    # This is the symlink tree \\
    --modules-dir=bin__link-tree
$ ./bin.pex
...
"""

import argparse
import os
import platform
import stat
from pathlib import Path

# Substituted for `<PYTHON>` in the template, i.e. it is what follows the `#!`.
#
# A shell/Python polyglot: `/bin/sh` runs the lines between `"true" '''` and the
# closing `'''`, while Python parses the same text as two implicitly concatenated
# string literals and keeps it as the module docstring. Windows has no `#!` support
# and invokes the bootstrapper as `python <pex>`, where it is likewise inert.
#
# `<REL_PYTHON>` is the interpreter's path relative to the bootstrapper.
_SH_TRAMPOLINE = r"""/bin/sh
"true" '''\'
# Resolve this script, following symlinks, so the interpreter can be found next to it.
_self=$0
case $_self in */*) ;; *) _self=./$_self ;; esac
_hops=0
while [ -L "$_self" ] && [ "$_hops" -lt 32 ]; do
    # `readlink` is not a shell builtin and `$PATH` may be scrubbed. Without it the
    # best we can do is assume the symlink sits alongside the interpreter.
    _link=$(readlink "$_self" 2>/dev/null) || break
    [ -n "$_link" ] || break
    case $_link in
        /*) _self=$_link ;;
        *) _self=${_self%/*}/$_link ;;
    esac
    _hops=$((_hops + 1))
done
exec "${_self%/*}/<REL_PYTHON>" "$0" "$@"
'''"""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Create a python inplace binary, writing a symlink tree to a directory, "
            "and a bootstrapper pex file to file"
        ),
        fromfile_prefix_chars="@",
    )
    parser.add_argument(
        "--template",
        required=True,
        type=Path,
        help="The template file for the .pex bootstrapper script",
    )
    parser.add_argument(
        "--preload",
        type=Path,
        dest="preload_libraries",
        action="append",
        default=[],
        help="A list of native libraries to add to LD_PRELOAD",
    )
    parser.add_argument(
        "--python",
        required=True,
        help=(
            "The python binary to launch the bootstrapper with. An absolute path or a"
            " bare name goes straight into the hashbang; a project-root-relative path"
            " is resolved at runtime against the bootstrapper, see launcher()"
        ),
    )
    parser.add_argument(
        "--host-python",
        required=True,
        help="The host python binary to use to e.g. compiling bytecode",
    )
    entry_point = parser.add_mutually_exclusive_group(required=True)
    entry_point.add_argument(
        "--entry-point",
        help="The main module to execute. Mutually exclusive with --main-function.",
    )
    entry_point.add_argument(
        "--main-function",
        help=(
            "Fully qualified name of the function that serves as the entry point."
            " Mutually exclusive with --entry-point."
        ),
    )
    parser.add_argument(
        "--main-runner",
        help=(
            "Fully qualified name of a function that handles invoking the"
            " executable's entry point."
        ),
        required=True,
    )
    parser.add_argument(
        "--modules-dir",
        required=True,
        type=Path,
        help="The link tree directory to use at runtime",
    )
    parser.add_argument(
        "output",
        type=Path,
        help="Where to write the bootstrapper script to",
    )
    parser.add_argument(
        "--native-libs-env-var",
        default=(
            "DYLD_LIBRARY_PATH" if platform.system() == "Darwin" else "LD_LIBRARY_PATH"
        ),
        help="The dynamic loader env used to find native library deps",
    )
    parser.add_argument(
        "--native-library-runtime-path",
        dest="native_library_runtime_paths",
        default=[],
        action="append",
        help="The dynamic loader env used to find native library deps",
    )
    parser.add_argument(
        "-e",
        "--runtime_env",
        action="append",
        default=[],
        help="environment variables to set before launching the runtime. (e.g. -e FOO=BAR BAZ=QUX)",
    )
    parser.add_argument(
        "--python-interpreter-flags",
        action="append",
        default=[],
        help="additional flags to pass to the Python interpreter (e.g. -Xgil=0)",
    )
    # Compatibility with existing make_par scripts
    parser.add_argument("--passthrough", action="append", default=[])
    # No-op, added for compatibility with existing make_par scripts
    parser.add_argument(
        "--omnibus-debug-info", choices=["separate", "strip", "extract"]
    )

    return parser.parse_args()


def launcher(python: str, output_dir: Path) -> str:
    """Build the body of the bootstrapper's `#!` line, i.e. how to start `python`.

    The result must not depend on where the project root happens to sit on this
    machine. The action's command line doesn't, so neither does its cache key, and an
    output holding a local absolute path would poison the cache for every other reader.
    """

    separators = (os.path.sep, os.path.altsep)
    if os.path.isabs(python) or not any(sep and sep in python for sep in separators):
        # An absolute path is machine-independent already, and a bare word is for
        # `/usr/bin/env` to look up on `$PATH`. Both work directly in a `#!` line.
        #
        # TODO(nmj): Remove this hack. So, if arg0 in your shebang is a bash script
        #                 (like /usr/local/fbcode/platform007/bin/python3.7 on macs is)
        #                 OSX just sort of ignores it and tries to run your thing with
        #                 the current shell. So, we hack in /usr/bin/env in the front
        #                 for now, and let it do the lifting. OSX: Bringing you the best
        #                 of 1980s BSD in 2021...
        return f"/usr/bin/env {python}"

    # The interpreter is a build artifact, so buck2 handed us a path relative to the
    # project root. A `#!` line can't carry one, because the kernel resolves it against
    # the caller's cwd, so defer to a shell that resolves it against the bootstrapper.
    relative_python = os.path.relpath(python, output_dir)
    return _SH_TRAMPOLINE.replace(
        "<REL_PYTHON>", relative_python.replace(os.path.sep, "/")
    )


def write_bootstrapper(args: argparse.Namespace) -> None:
    """Write the .pex bootstrapper script using a template"""

    with open(args.template, "r", encoding="utf8") as fin:
        data = fin.read()

    # Because this can be invoked from other directories, find the relative path
    # from this .par to the modules dir, and use that.
    relative_modules_dir = os.path.relpath(args.modules_dir, args.output.parent)
    native_lib_dirs = [relative_modules_dir] + args.native_library_runtime_paths

    ld_preload = None
    if args.preload_libraries:
        ld_preload = [p.name for p in args.preload_libraries]

    new_data = data.replace("<PYTHON>", launcher(str(args.python), args.output.parent))
    # Interpreter flags go to the Python variable that the re-exec path reads (e.g.
    # -Xgil=0 for free-threaded builds) rather than onto the `#!` line, which on Linux
    # can carry at most one argument.
    new_data = new_data.replace(
        "<PYTHON_INTERPRETER_FLAGS>", " ".join(args.python_interpreter_flags or [])
    )

    new_data = new_data.replace("<MODULES_DIR>", str(relative_modules_dir))
    main_module = args.entry_point
    main_function = ""
    if args.main_function:
        main_module, main_function = args.main_function.rsplit(".", 1)
    new_data = new_data.replace("<MAIN_MODULE>", main_module)
    new_data = new_data.replace("<MAIN_FUNCTION>", main_function)

    main_runner_module, main_runner_function = args.main_runner.rsplit(".", 1)
    new_data = new_data.replace("<MAIN_RUNNER_MODULE>", main_runner_module)
    new_data = new_data.replace("<MAIN_RUNNER_FUNCTION>", main_runner_function)

    # Things that are only required for the full template
    new_data = new_data.replace("<NATIVE_LIBS_ENV_VAR>", args.native_libs_env_var)
    new_data = new_data.replace("<NATIVE_LIBS_DIRS>", repr(native_lib_dirs))
    new_data = new_data.replace("<NATIVE_LIBS_PRELOAD_ENV_VAR>", "LD_PRELOAD")
    new_data = new_data.replace("<NATIVE_LIBS_PRELOAD>", repr(ld_preload))

    if args.runtime_env:
        runtime_env = dict(e.split("=", maxsplit=1) for e in args.runtime_env)
        env = f"os.environ.update({runtime_env!r})"
    else:
        env = ""
    new_data = new_data.replace("<ENV>", env)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with open(args.output, "w", encoding="utf8") as fout:
        fout.write(new_data)
    mode = os.stat(args.output).st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
    os.chmod(args.output, mode)


def main() -> None:
    args = parse_args()
    write_bootstrapper(args)


if __name__ == "__main__":
    main()
