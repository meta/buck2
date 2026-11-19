#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Lists the modules of a Lean target and reads their imports.

Usage: lean_deps.py --out=deps.json [--src=FILE]... [--src-dir=DIR]... lean [args...]

The modules are the `--src` files, then the `.lean` files below each
`--src-dir`, in sorted order; other files below a `--src-dir` are ignored,
and a `--src-dir` with no `.lean` file is an error. `lean --deps-json --stdin`
reads the imports of all of them: the file names go through stdin, so their
number has no limit.

The output is JSON. `dirs` has one list per `--src-dir`: its files, relative
to it, with `/` as the separator. `imports` is the `imports` of
`lean --deps-json`, one entry per module, in the order above.
"""

import argparse
import json
import os
import subprocess
import sys


def lean_files(src_dir: str) -> list[str]:
    files = []
    for dirpath, dirnames, filenames in os.walk(src_dir):
        dirnames.sort()
        for filename in sorted(filenames):
            if filename.endswith(".lean"):
                path = os.path.relpath(os.path.join(dirpath, filename), src_dir)
                files.append(path.replace(os.sep, "/"))
    return files


def main() -> int:
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument("--out", required=True)
    parser.add_argument("--src", action="append", default=[])
    parser.add_argument("--src-dir", action="append", default=[])
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    files = list(args.src)
    dirs = []
    for src_dir in args.src_dir:
        if not os.path.isdir(src_dir):
            print(f"lean_deps: `{src_dir}` is not a directory", file=sys.stderr)
            return 1
        paths = lean_files(src_dir)
        if not paths:
            print(f"lean_deps: no `.lean` file below `{src_dir}`", file=sys.stderr)
            return 1
        dirs.append(paths)
        files += [src_dir + "/" + path for path in paths]

    result = subprocess.run(
        args.command + ["--stdin"],
        input="".join(file + "\n" for file in files).encode("utf-8"),
        stdout=subprocess.PIPE,
    )
    if result.returncode != 0:
        return result.returncode
    imports = json.loads(result.stdout)["imports"]
    with open(args.out, "w", encoding="utf-8") as out:
        json.dump({"dirs": dirs, "imports": imports}, out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
