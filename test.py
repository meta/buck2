#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Fake script that acts as a test
"""

import argparse
import os
import shlex
import signal
import subprocess
import sys
import time
from collections.abc import Generator, Iterable
from contextlib import contextmanager
from enum import Enum
from pathlib import Path
from typing import Optional


def is_windows() -> bool:
    return sys.platform == "win32"


class Colors(Enum):
    # Copied from https://stackoverflow.com/questions/287871/how-to-print-colored-text-to-the-terminal
    HEADER = "\033[95m"
    OKBLUE = "\033[94m"
    OKCYAN = "\033[96m"
    OKGREEN = "\033[92m"
    WARNING = "\033[93m"
    FAIL = "\033[91m"
    ENDC = "\033[0m"
    BOLD = "\033[1m"
    UNDERLINE = "\033[4m"


def print_running(msg: str) -> None:
    print(Colors.OKGREEN.value + "Running " + msg + Colors.ENDC.value)


def print_error(msg: str) -> None:
    print(
        Colors.FAIL.value + Colors.BOLD.value + "ERROR: " + msg + Colors.ENDC.value,
        file=sys.stderr,
    )


def print_warn(msg: str) -> None:
    print(
        Colors.WARNING.value
        + Colors.BOLD.value
        + "WARNING: "
        + msg
        + Colors.ENDC.value,
        file=sys.stderr,
    )


@contextmanager
def timing() -> Generator:
    start = time.time()
    yield
    duration = time.time() - start
    print(f"Finished in {duration:.2f} seconds.")


def run(
    args: Iterable[str],
    capture_output: bool = False,
    env: Optional[dict[str, str]] = None,
    timeout: Optional[int] = None,
) -> subprocess.CompletedProcess:
    """
    Runs a command (args) in a new process.
    If the command fails, raise CalledProcessError.
    If the command passes, return CompletedProcess.
    If capture_output is False, print to the console, otherwise record it as CompletedProcess.stdout/stderr.
    If error is specified, print error on stderr when there is a CalledProcessError.
    """
    # On Ci stderr gets out of order with stdout. To avoid this, we need to flush stdout/stderr first.
    args = tuple(args)
    print(f"Running {shlex.join(args)}", file=sys.stdout)
    sys.stdout.flush()
    sys.stderr.flush()
    try:
        result = subprocess.run(
            args,
            # We'd like to use the capture_output argument,
            # but that isn't available in Python 3.6 which we use on Windows
            stdout=subprocess.PIPE if capture_output else sys.stdout,
            stderr=sys.stderr,
            check=True,
            encoding="utf-8",
            env=env or os.environ.copy(),
            timeout=timeout,
        )
        return result
    except subprocess.CalledProcessError as e:
        # Print the console info if we were capturing it
        if capture_output:
            print(e.stdout, file=sys.stdout)
        sys.exit(1)


def check_no_changes(git: bool):
    status_cmd = []
    diff_cmd = []
    if git:
        status_cmd = ["git", "status", "--porcelain"]
        diff_cmd = ["git", "diff"]

    else:
        status_cmd = ["hg", "status", "-mard"]
        diff_cmd = ["hg", "diff", "--pager=none"]

    status = run(status_cmd, capture_output=True)
    if status.stdout.strip():
        run(status_cmd)
        run(diff_cmd)
        print_error(
            "File changes! Caused either by formatting or by tests creating stray files."
        )
        sys.exit(1)


def clippy(package_args: list[str], fix: bool, target_args: list[str]) -> None:
    """
    Run cargo clippy, failing on any warning. Which lints warn is the
    workspace's `[workspace.lints]`, the same levels Buck applies.
    """

    print_running("clippy")

    clippy_fix_args = ["--fix"] if fix else []

    run(
        [
            "cargo",
            "clippy",
            "--keep-going",
            *package_args,
            *target_args,
            *clippy_fix_args,
            "--profile=test",
            "--tests",
            "--benches",
            "--",
            "--deny=warnings",
        ]
    )


def rustdoc(package_args: list[str], target_args: list[str]) -> None:
    print_running("cargo doc")
    env = os.environ.copy()
    env["RUSTDOCFLAGS"] = " ".join(
        filter(None, [env.get("RUSTDOCFLAGS"), "--deny=warnings"])
    )
    run(["cargo", "doc", "--no-deps", *package_args, *target_args], env=env)


def test(package_args: list[str], target_args: list[str]) -> None:
    print_running("cargo test --lib")
    extra_args = []
    # Limit number of parallel jobs to prevent OOMs
    if is_windows():
        extra_args = ["--jobs", str(os.cpu_count() // 2)]
    # Hour should be enough for all tests to run
    timeout_sec = 60 * 60
    run(
        ["cargo", "test", "--lib", *extra_args, *package_args, *target_args],
        timeout=timeout_sec,
    )
    print_running("cargo test --doc")
    run(
        ["cargo", "test", "--doc", *extra_args, *package_args, *target_args],
        timeout=timeout_sec,
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--ci",
        action="store_true",
        default=False,
        help="Whether to run CI workflow",
    )
    parser.add_argument(
        "--git",
        action="store_true",
        default=False,
        help="Use `git` to check repo state, the script defaults to `hg`",
    )
    parser.add_argument(
        "--lint-only",
        action="store_true",
        default=False,
        help="Run clippy only. Do not run rustdoc or tests.",
    )
    parser.add_argument(
        "--rustdoc-only",
        action="store_true",
        default=False,
        help="Run rustdoc only. Do not run clippy or tests.",
    )
    parser.add_argument(
        "--test-only",
        action="store_true",
        default=False,
        help="Run tests only. Do not run clippy or rustdoc.",
    )
    parser.add_argument(
        "--exclude",
        action="append",
        help="Packages excluded from linting.",
    )
    parser.add_argument(
        "--clippy-fix",
        action="store_true",
        default=False,
        help="Apply Clippy suggestions",
    )
    parser.add_argument(
        "--toolchain-target",
        action="store",
        default=None,
        help="Target triple passed to cargo via --target (e.g. x86_64-pc-windows-gnu)",
    )
    parser.add_argument(
        "packages",
        nargs="*",
        type=str,
        help="The packages to run lint on. If not specified, all packages",
    )
    args = parser.parse_args()

    # Change to buck2 directory
    buck2_dir = Path(__file__).parent.absolute()
    os.chdir(str(buck2_dir))

    package_args = [f"--package={p.rstrip('/')}" for p in args.packages]
    if args.exclude:
        package_args.append("--workspace")
        package_args.extend([f"--exclude={p.rstrip('/')}" for p in args.exclude])

    target_args = ["--target", args.toolchain_target] if args.toolchain_target else []

    if not (args.rustdoc_only or args.test_only):
        with timing():
            clippy(package_args, args.clippy_fix, target_args)

    if not (args.lint_only or args.test_only):
        with timing():
            rustdoc(package_args, target_args)

    if not (args.lint_only or args.rustdoc_only):
        with timing():
            test(package_args, target_args)

    # On CI, check to make sure our test doesn't overwrite existing files
    if args.ci:
        check_no_changes(args.git)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        # no stack trace on interrupt
        sys.exit(signal.SIGINT)
