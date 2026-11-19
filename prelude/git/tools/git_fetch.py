# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import argparse
import os
import shutil
import subprocess
import sys
import time
from enum import Enum
from pathlib import Path
from typing import List, NamedTuple, Optional


MAX_RETRIES = 5

_FALLBACK_GIT_PATHS = [
    "/usr/bin/git",
    "/usr/local/bin/git",
]


def _find_git(explicit: Optional[str] = None) -> str:
    if explicit:
        return explicit
    found = shutil.which("git")
    if found:
        return found
    for path in _FALLBACK_GIT_PATHS:
        if Path(path).is_file():
            return path
    return "git"


def run(cmd: List[str], check: bool, retries: int = MAX_RETRIES) -> str:
    print(f"Running {cmd}", file=sys.stderr)
    try:
        proc = subprocess.run(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            encoding="utf-8",
            check=check,
        )
    except OSError as ex:
        print(ex, file=sys.stderr)
        sys.exit(1)
    except subprocess.CalledProcessError as ex:
        print(ex.stderr, file=sys.stderr)
        if "The requested URL returned error: 429" in ex.stderr and retries > 0:
            time.sleep(2 ** (MAX_RETRIES - retries))
            return run(cmd, check, retries - 1)
        sys.exit(ex.returncode)
    return proc.stdout


class ObjectFormat(str, Enum):
    Sha1 = "sha1"
    Sha256 = "sha256"


class Args(NamedTuple):
    git: Optional[str]
    git_dir: Path
    work_tree: Path
    object_format: ObjectFormat
    repo: str
    rev: str
    update_submodules: bool


def arg_parse() -> Args:
    parser = argparse.ArgumentParser()
    parser.add_argument("--git", type=str, required=False, default=None)
    parser.add_argument("--git-dir", type=Path, required=True)
    parser.add_argument("--work-tree", type=Path, required=True)
    parser.add_argument("--object-format", type=ObjectFormat, required=False)
    parser.add_argument("--update-submodules", action="store_true", required=False)
    parser.add_argument("--repo", type=str, required=True)
    parser.add_argument("--rev", type=str, required=True)
    return Args(**vars(parser.parse_args()))


def git_configure(git: List[str]):
    # Override if someone has `[core] autocrlf = true` in their global gitconfig
    # on a Windows machine. That causes LF to be converted into CRLF when
    # checking out text files, which is not desired here. Unix and Windows
    # should produce identical directory as output.
    run([*git, "config", "core.autocrlf", "false"], check=False)


def main() -> None:
    args = arg_parse()

    args.work_tree.mkdir(exist_ok=True)

    git_bin = _find_git(args.git)
    git = [git_bin, f"--git-dir={args.git_dir}", f"--work-tree={args.work_tree}"]
    if args.object_format is None:
        object_format_args = []
    else:
        object_format_args = ["--object-format", args.object_format]

    run([*git, "init"] + object_format_args, check=True)
    git_configure(git)
    run([*git, "remote", "remove", "origin"], check=False)
    run([*git, "remote", "add", "origin", args.repo], check=True)
    run([*git, "fetch", "--depth=1", "origin", args.rev], check=True)

    fetch_head = run([*git, "rev-parse", "FETCH_HEAD"], check=True)
    fetch_head = fetch_head.strip()
    if fetch_head != args.rev:
        raise RuntimeError(
            f"fetched the wrong commit: expected {args.rev}, fetched {fetch_head}"
        )

    run([*git, "checkout", "FETCH_HEAD"], check=True)

    if args.update_submodules:
        git_in_worktree = [
            git_bin,
            f"--git-dir={os.path.abspath(args.git_dir)}",
            # `git submodule` does not accept `--work-tree`, so run it from the worktree.
            "-C",
            str(args.work_tree),
        ]
        run(
            [
                *git_in_worktree,
                "submodule",
                "update",
                "--init",
                "--recursive",
                "--depth=1",
            ],
            check=True,
        )


if __name__ == "__main__":
    main()
