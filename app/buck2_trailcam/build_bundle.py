#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Builds the Trailcam viewer bundle from its sources and packs it into the
tarball that `rust_linkable_symbol` links into buck2.

Python rather than a shell `cmd` in the genrule because buck2 builds on
Windows too. Same shape as `buck2_explain/js/build_html.py`.
"""

import argparse
import os
import shlex
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
from typing import List


def run(command: List[str], env: dict) -> None:
    # Buck hands out the tool paths relative to the genrule's working
    # directory, so the current directory stays put; yarn gets `--cwd`.
    print(f"$ {shlex.join(command)}", file=sys.stderr)
    subprocess.run(command, env=env, check=True, shell=(os.name == "nt"))


def copy_writable(src: str, dst: str, *, follow_symlinks: bool = True) -> None:
    """Buck materializes sources read-only; yarn and vite need to write next to them."""
    shutil.copy(src, dst, follow_symlinks=follow_symlinks)
    os.chmod(dst, os.stat(dst).st_mode | stat.S_IWUSR)


def write_tar(dist: str, output: str) -> None:
    """Pack `dist` into `output`, byte-for-byte reproducible for the same inputs.

    Source maps are left out: they are three quarters of the directory and
    only matter when debugging the bundle itself.
    """
    with tarfile.open(output, "w", format=tarfile.GNU_FORMAT) as tar:
        for root, dirs, files in os.walk(dist):
            dirs.sort()
            for name in sorted(files):
                if name.endswith(".map"):
                    continue
                path = os.path.join(root, name)
                info = tar.gettarinfo(
                    path, arcname=os.path.relpath(path, dist).replace(os.sep, "/")
                )
                info.mtime = 0
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mode = 0o644
                with open(path, "rb") as f:
                    tar.addfile(info, f)


def main() -> None:
    parser = argparse.ArgumentParser(description="Build the Trailcam bundle tarball.")
    parser.add_argument(
        "--yarn", required=True, help="Yarn executable (may include arguments)."
    )
    parser.add_argument(
        "--node", required=True, help="Node launcher; its directory goes on PATH."
    )
    parser.add_argument(
        "--yarn-offline-mirror",
        required=True,
        help="Offline mirror view for the lockfile.",
    )
    parser.add_argument(
        "--src",
        required=True,
        help="The core_srcs filegroup output; the package is at <src>/core.",
    )
    parser.add_argument("--tmp", required=True, help="Scratch directory for the build.")
    parser.add_argument(
        "-o", "--output", required=True, help="Path of the tarball to write."
    )
    args = parser.parse_args()

    yarn = shlex.split(args.yarn, posix=False)
    work = tempfile.mkdtemp(prefix="trailcam_bundle", dir=args.tmp)
    try:
        shutil.copytree(
            os.path.join(args.src, "core"),
            work,
            dirs_exist_ok=True,
            copy_function=copy_writable,
        )
        env = dict(os.environ)
        env["YARN_YARN_OFFLINE_MIRROR"] = os.path.realpath(args.yarn_offline_mirror)
        env["YARN_CACHE_FOLDER"] = os.path.join(work, ".yarn-cache")
        env["PATH"] = (
            os.path.dirname(os.path.realpath(args.node))
            + os.pathsep
            + env.get("PATH", "")
        )
        run(
            yarn
            + [
                "--cwd",
                work,
                "install",
                "--offline",
                "--frozen-lockfile",
                "--ignore-scripts",
                "--check-files",
                "--non-interactive",
            ],
            env=env,
        )
        dist = os.path.join(work, "dist")
        run(yarn + ["--cwd", work, "run", "build", "--outDir", dist], env=env)
        write_tar(dist, args.output)
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
