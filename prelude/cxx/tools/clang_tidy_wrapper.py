#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Wrapper that generates a compile_commands.json and runs clang-tidy.

Usage:
    clang_tidy_wrapper.py \\
        --output=path/to/diagnostics.txt \\
        --clang-tidy=path/to/clang-tidy \\
        --source=path/to/file.cpp \\
        -- <compile_command> [args...]

The compile command after -- is the same as what make_comp_db.py gen receives:
the compiler binary followed by all flags and argsfiles. This wrapper expands
argsfiles, filters out fbcc-specific flags (e.g. --cc, --log-fbcc), writes a
single-entry compile_commands.json to a temporary directory, and invokes
clang-tidy with -p pointing to that directory.
"""

from __future__ import annotations

import json
import os
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path

# ---------------------------------------------------------------------------
# fbcc flag filtering (inlined from lib/FbccFilterArgs.py)
# ---------------------------------------------------------------------------

_JOINED_PREFIX_FLAGS: tuple[str, ...] = (
    "--cc=",
    "--fbcc-create-external-debug-info=",
    "--log-fbcc=",
    "-fbcc-opts=",
)

_STANDALONE_FLAGS: tuple[str, ...] = ("--show-flags",)

_XFBCC_APPEND_FLAG = "-XfbccAppend"


def _filter_fbcc_args(args: list[str]) -> list[str]:
    """Return *args* with all fbcc-specific flags removed.

    ``-XfbccAppend <value>`` consumes two tokens; the value is appended to the
    end of the returned list (mirroring fbcc's own reordering behaviour).
    """
    filtered: list[str] = []
    appended: list[str] = []
    it = iter(args)
    for arg in it:
        if any(arg.startswith(p) for p in _JOINED_PREFIX_FLAGS):
            continue
        if arg in _STANDALONE_FLAGS:
            continue
        if arg == _XFBCC_APPEND_FLAG:
            value = next(it, None)
            if value is not None:
                appended.append(value)
            continue
        filtered.append(arg)
    filtered.extend(appended)
    return filtered


# ---------------------------------------------------------------------------
# Argsfile expansion
# ---------------------------------------------------------------------------


def expand_argsfiles(arguments: list[str]) -> list[str]:
    """Expand @argsfile references recursively."""
    result = []
    for arg in arguments:
        if arg.startswith("@"):
            with open(arg[1:]) as f:
                tokens = []
                for line in f:
                    tokens.extend(shlex.split(line))
                result.extend(expand_argsfiles(tokens))
        else:
            result.append(arg)
    return result


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    argv = sys.argv[1:]
    try:
        sep_idx = argv.index("--")
        our_args = argv[:sep_idx]
        compile_command = argv[sep_idx + 1 :]
    except ValueError:
        print("error: missing -- separator", file=sys.stderr)
        return 1

    output: Path | None = None
    clang_tidy: str | None = None
    source: str | None = None

    i = 0
    while i < len(our_args):
        arg = our_args[i]
        if arg.startswith("--output="):
            output = Path(arg.split("=", 1)[1])
        elif arg.startswith("--clang-tidy="):
            clang_tidy = arg.split("=", 1)[1]
        elif arg.startswith("--source="):
            source = arg.split("=", 1)[1]
        i += 1

    if not output or not clang_tidy or not source:
        print(
            "error: --output, --clang-tidy, and --source are required",
            file=sys.stderr,
        )
        return 1

    expanded_args = expand_argsfiles(compile_command)
    expanded_args = _filter_fbcc_args(expanded_args)

    source_abspath = os.path.abspath(source)
    entry = [
        {
            "file": source_abspath,
            "directory": os.getcwd(),
            "arguments": expanded_args,
        },
    ]

    with tempfile.TemporaryDirectory() as tmpdir:
        comp_db_path = os.path.join(tmpdir, "compile_commands.json")
        with open(comp_db_path, "w") as f:
            json.dump(entry, f)

        cmd = [
            clang_tidy,
            "-p",
            tmpdir,
            source,
        ]

        result = subprocess.run(
            cmd,
            capture_output=True,
        )

    diagnostics = result.stdout + result.stderr
    output.write_bytes(diagnostics)
    sys.stderr.buffer.write(diagnostics)
    sys.stderr.flush()

    return result.returncode


sys.exit(main())
