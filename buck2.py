#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# This script builds buck2 locally from source and then runs it.
#
# It uses the installed buck2 to build the buck2 bundle target, then executes
# the freshly built binary with any additional arguments passed to this script.
# This is useful for testing local changes to buck2 without a full installation.
#
# Example:
#   ./buck2.py build //my:target

import argparse
import os
import platform
import subprocess
import sys
from typing import List, Optional, Tuple


def parse_arguments() -> Tuple[argparse.Namespace, List[str]]:
    parser = argparse.ArgumentParser(
        add_help=False,  # This allows us to pass --help to the inner command
        description="Builds buck2 locally and then runs it.",
        formatter_class=argparse.RawTextHelpFormatter,
    )
    parser.add_argument(
        "--run-isolation-dir",
        type=str,
        default="",
        help="Isolation dir for the inner command",
    )
    parser.add_argument(
        "--echo-run-cmd",
        action="store_true",
        help="Echo the run command before executing",
    )
    parser.add_argument(
        "--canary-stable-tpx",
        type=str,
        default="",
        help=(
            "LOCAL CANARY ONLY. Build buck2-tpx once to this stable path and run "
            "the installed buck2 with test.v2_test_executor pinned to it, instead "
            "of building+running a local buck2 bundle. This keeps the locally-built "
            "TPX alive across Citadel's HEAD->BASE rebase (the target leg vs the "
            "BRR/SRR legs), which otherwise GCs the bundle's buck-out copy and "
            "fails with 'v2_test_executor ... binary has been deleted from disk'. "
            "Inert when empty."
        ),
    )
    return parser.parse_known_args()


def get_extra_build_params(args: argparse.Namespace) -> List[str]:
    system_platform = platform.system()
    if system_platform == "Windows":
        return ["@fbcode//mode/opt-win"]

    params = ["-m", "opt"]

    arch_platform = platform.machine()
    if arch_platform == "x86_64":
        params.extend(["-m", "x86_64"])
    elif arch_platform == "arm64":
        params.extend(["-m", "arm64"])
    elif arch_platform == "riscv64":
        params.extend(["-m", "riscv64"])

    return params


def build_command(
    args: argparse.Namespace, extra_args: List[str], cwd: Optional[str]
) -> List[str]:
    cmd = [
        "buck2",
        "run",
        # @oss-disable[end= ]: "fbcode//buck2:buck2_bundle",
        "//:buck2_bundle", # @oss-enable
    ]
    inner_buck_isolation_dir = (
        args.run_isolation_dir if args.run_isolation_dir else "v2.self"
    )
    inner_buck_isolation_dir_arg = [f"--isolation-dir={inner_buck_isolation_dir}"]

    cmd.extend(get_extra_build_params(args))
    if cwd is not None and "--chdir" not in extra_args:
        cmd.extend(["--chdir", os.getcwd()])

    cmd.append("--")
    cmd.extend(inner_buck_isolation_dir_arg)
    cmd.extend(extra_args)

    if args.echo_run_cmd:
        print(" ".join(cmd))

    return cmd


# Target whose default output is the TPX test executor that the buck2 bundle
# materialises as "buck2-tpx" (see fbcode/buck2/defs.bzl).
TPX_TARGET = "fbcode//buck2/buck2_tpx_cli:buck2_tpx_cli"


def tpx_build_mode() -> str:
    # opt is intentional: a mode/dev TPX fails to run from outside buck-out
    # (missing libunwind.so.8), whereas the opt binary is self-contained.
    return (
        "@fbcode//mode/opt-win"
        if platform.system() == "Windows"
        else "@fbcode//mode/opt"
    )


def ensure_stable_tpx(stable_path: str) -> None:
    """Build buck2-tpx to `stable_path` once (idempotent across Citadel legs).

    The build runs against the current checkout, so the first (target/HEAD) leg
    captures the local TPX changes; later legs reuse the same binary after the
    rebase to BASE.
    """
    if os.path.exists(stable_path):
        return
    parent = os.path.dirname(stable_path) or "."
    os.makedirs(parent, exist_ok=True)
    building = stable_path + ".building"
    if os.path.exists(building):
        os.remove(building)
    subprocess.run(
        ["buck2", "build", tpx_build_mode(), TPX_TARGET, "--out", building],
        check=True,
    )
    os.chmod(building, 0o755)
    os.replace(
        building, stable_path
    )  # atomic publish so a half-built binary is never used


def run_with_stable_tpx(stable_path: str, extra_args: List[str]) -> None:
    ensure_stable_tpx(stable_path)
    override = ["-c", f"test.v2_test_executor={stable_path}"]
    # Insert the override right after the buck subcommand (e.g. `test`) so it
    # lands before any `--` test-runner separator.
    if extra_args:
        cmd = ["buck2", extra_args[0], *override, *extra_args[1:]]
    else:
        cmd = ["buck2", *override]
    result = subprocess.run(cmd)
    sys.exit(result.returncode)


def main() -> None:
    args, extra_args = parse_arguments()
    if args.canary_stable_tpx:
        run_with_stable_tpx(args.canary_stable_tpx, extra_args)
        return
    cwd = None
    if __file__ is not None:
        cwd = os.path.dirname(__file__)
    cmd = build_command(args, extra_args, cwd)
    result = subprocess.run(cmd, cwd=cwd)
    sys.exit(result.returncode)


if __name__ == "__main__":
    main()
