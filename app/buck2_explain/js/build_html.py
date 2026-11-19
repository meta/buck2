#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


# credits to implementation in https://www.internalfb.com/code/fbsource/fbcode/eden/addons/build-tar.py

import argparse
import atexit
import functools
import glob
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
from typing import cast, List, Protocol

CSS_LINK = '<link href="dist/App.css" rel="stylesheet" />'
JS_SCRIPT = '<script src="dist/App.js"></script>'


class RmtreeProtocol(Protocol):
    def __call__(self, path: str, *, ignore_errors: bool) -> None: ...


rm_rf = functools.partial(cast(RmtreeProtocol, shutil.rmtree), ignore_errors=True)
print_err = functools.partial(print, file=sys.stderr)
glob_r = functools.partial(glob.glob, recursive=True)


def run(command: List[str], cwd=None, env=None) -> None:
    print_err(f"{cwd if cwd else ' '} $ {shlex.join(command)}")

    if env is not None:
        env = {**os.environ, **env}

    # shell=True with a List `command` seems buggy on *nix.
    # It might run ['sh', '-c', 'a', 'b'] instead of ['sh', '-c', 'a b'].
    subprocess.run(command, shell=(os.name == "nt"), check=True, cwd=cwd, env=env)


def copy_writable(src, dst, *, follow_symlinks=True) -> None:
    """shutil.copy, but ensure that yarn.lock is writable
    - RE might make src/ read-only with its "restrictive mode".
    - When copying the RE "restrictive" src/, yarn.lock is read-only.
    - yarn wants yarn.lock to be writable, even with --frozen-lockfile.
    """
    shutil.copy(src, dst, follow_symlinks=follow_symlinks)
    if dst.endswith("yarn.lock") and os.name != "nt":
        os.chmod(dst, 0o666)


def main() -> None:
    parser = argparse.ArgumentParser(description="Creates a html from explain source.")
    parser.add_argument(
        "-o",
        "--output",
        nargs="?",
        help="Path to the output '.html' file.",
    )
    parser.add_argument(
        "--yarn",
        help="Path to yarn executable.",
    )
    parser.add_argument(
        "--yarn-offline-mirror",
        help="Path to the yarn offline mirror.",
    )
    parser.add_argument(
        "--src",
        help="Directory that contains the source code.",
    )
    parser.add_argument(
        "--tmp",
        help="Temporary directory to run build. Do not modify src in-place.",
    )

    args = parser.parse_args()

    # posix=False prevents shlex.split from treating \\ as escape character, breaking Windows.
    yarn = shlex.split(args.yarn, posix=False)

    # copy source to a temporary directory
    # used by buck genrule, which does not guarantee src is writable
    tmp_src_path = tempfile.mkdtemp(prefix="explain_src", dir=args.tmp)
    atexit.register(lambda: rm_rf(tmp_src_path))
    print_err(f"copying source {args.src} to {tmp_src_path}")
    shutil.copytree(
        args.src, tmp_src_path, dirs_exist_ok=True, copy_function=copy_writable
    )

    offline_mirror = args.yarn_offline_mirror
    # Resolving Buck's symlinked artifact points into Eden scratch on Windows.
    if os.name != "nt":
        offline_mirror = os.path.realpath(offline_mirror)
    env = {"YARN_YARN_OFFLINE_MIRROR": offline_mirror}
    # Yarn's default cache is shared by every yarn process on the host and has
    # no cross-process locking, so concurrent installs corrupt it.
    with tempfile.TemporaryDirectory(
        prefix="yarn_cache", dir=args.tmp, ignore_cleanup_errors=True
    ) as yarn_cache:
        run(
            yarn
            + [
                "--cwd",
                tmp_src_path,
                "install",
                "--offline",
                "--frozen-lockfile",
                "--ignore-scripts",
                "--check-files",
                "--cache-folder",
                yarn_cache,
            ],
            env=env,
        )

    rm_rf(os.path.join(tmp_src_path, "dist"))

    # build
    run(yarn + ["--cwd", tmp_src_path, "run", "build"], env={"CI": "false"})

    # inline js and css into html file
    with open(os.path.join(tmp_src_path, "dist/App.js"), "r") as f:
        js_content = f.read()
    with open(os.path.join(tmp_src_path, "dist/App.css"), "r") as f:
        css_content = f.read()
    with open(os.path.join(tmp_src_path, "index.html"), "r") as f:
        html_content = f.read()

    assert JS_SCRIPT in html_content
    assert CSS_LINK in html_content
    html_content = html_content.replace(CSS_LINK, f"<style>{css_content}</style>")
    html_content = html_content.replace(JS_SCRIPT, f"<script>{js_content}</script>")

    with open(args.output, "w") as out_file:
        out_file.write(html_content)


if __name__ == "__main__":
    main()
