#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Wrapper for rustc (or similar, like rustdoc). This wrapper does a few pieces
# of post-processing on the json-formatted diagnostics:
# - (preprocessing) resolve env vars referring to paths to absolute paths
# - write the rendered form to a text diagnostic output, and also to stderr
# - annotate unused crate messages with buck target info for downstream tooling,
#   and also generated a rendered version
# - generate a build status json when using failure filtering
#
# This is closely coupled to `_rustc_invoke` in `build.bzl`

import argparse
import asyncio
import json
import os
import platform
import re
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, IO, NamedTuple, Optional


# Sentinel used to mark OUT_DIR-relative paths emitted by buildscripts.
# We later replace this sentinel with the actual content-addressed path, once that is known.
OUT_DIR_SENTINEL: str = "${__BUILDSCRIPT_OUT_DIR__}"

DEBUG = False

INHERITED_ENV = [
    "RUSTC_LOG",
    "RUST_BACKTRACE",
    "PATH",
    "PWD",
    "HOME",
    "RUSTUP_HOME",
    "TMPDIR",
    # Required on Windows
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "TEMP",
    "TMP",
    # TODO(andirauter): Required by RE. Remove them when no longer required T119466023
    "EXECUTION_ID",
    "SESSION_ID",
    "ACTION_DIGEST",
    "RE_PLATFORM",
    "BUDGET_ENTITY",
    "CAS_DAEMON_PORT",
    "CAS_DAEMON_ADDR",
    # Required by Dotslash, which is how the Rust toolchain is shipped on Mac.
    "USER",
    "DOTSLASH_CACHE",
    # Required to run Python on Windows (for linker wrapper).
    "SYSTEMROOT",
    # Required on Windows for getpass.getuser() to work.
    "USERNAME",
    # Option to disable hg pre-fork client.
    # We might pass it to avoid long-running process created inside a per-action cgroup.
    # Such long-running process make it impossible to clean up systemd slices.
    # Context https://fb.workplace.com/groups/mercurialusers/permalink/2901424916673036/
    "CHGDISABLE",
    # Nix
    "NIX_BINTOOLS",
    "NIX_BINTOOLS_FOR_TARGET",
    "NIX_BINTOOLS_WRAPPER_TARGET_HOST_*",
    "NIX_BINTOOLS_WRAPPER_TARGET_TARGET_*",
    "NIX_CC",
    "NIX_CC_FOR_TARGET",
    "NIX_CC_WRAPPER_TARGET_HOST_*",
    "NIX_CC_WRAPPER_TARGET_TARGET_*",
    "NIX_CFLAGS_COMPILE",
    "NIX_CFLAGS_COMPILE_FOR_TARGET",
    "NIX_COREFOUNDATION_RPATH",
    "NIX_DEBUG",
    "NIX_DONT_SET_RPATH",
    "NIX_DONT_SET_RPATH_FOR_BUILD",
    "NIX_ENFORCE_NO_NATIVE",
    "NIX_HARDENING_ENABLE",
    "NIX_IGNORE_LD_THROUGH_GCC",
    "NIX_LDFLAGS",
    "NIX_LDFLAGS_FOR_TARGET",
    "NIX_NO_SELF_RPATH",
]


def eprint(*args: Any, **kwargs: Any) -> None:
    print(*args, end="\n", file=sys.stderr, flush=True, **kwargs)


if sys.version_info[:2] < (3, 7):
    eprint("Python 3.7 or newer is required!")
    eprint(f"Using {platform.python_version()} from {sys.executable}")
    sys.exit(1)


def key_value_arg(s: str) -> tuple[str, str]:
    return plain_key_value_arg(arg_eval(s))


def plain_key_value_arg(s: str) -> tuple[str, str]:
    """Splits without evaluating `$(`: `--env` values are decoded and evaluated in `main`."""
    key_value = s.split("=", maxsplit=1)
    if len(key_value) == 2:
        return (key_value[0], key_value[1])
    raise argparse.ArgumentTypeError(f"expected the form `key=value` for `{s}`")


_ENV_ESCAPE_RE = re.compile(r"\\(?:[\\nrvf]|x[0-9a-fA-F]{2}|u[0-9a-fA-F]{4})")
_ENV_ESCAPES = {"\\": "\\", "n": "\n", "r": "\r", "v": "\x0b", "f": "\x0c"}


def _decode_env_escape(match: re.Match[str]) -> str:
    escape = match.group()[1:]
    if escape[0] in "xu":
        return chr(int(escape[1:], 16))
    return _ENV_ESCAPES[escape]


def unescape_env_value(value: str) -> str:
    """Undo the escaping `process_env` in build.bzl applies so that a value fits on one line
    of the argfile: `\\\\`, `\\n`, `\\r`, `\\v`, `\\f`, `\\xHH` and `\\uHHHH`, decoded in one
    left-to-right pass. A backslash that starts no such escape is kept as it is."""
    return _ENV_ESCAPE_RE.sub(_decode_env_escape, value)


class Args(NamedTuple):
    diag_json: Optional[IO[bytes]]
    diag_txt: Optional[IO[bytes]]
    passthrough_stderr: bool
    env: Optional[list[tuple[str, str]]]
    path_env: Optional[list[tuple[str, str]]]
    remap_cwd_prefix: Optional[str]
    crate_map: Optional[list[tuple[str, str]]]
    buck_target: Optional[str]
    failure_filter: Optional[IO[bytes]]
    required_output: Optional[list[tuple[str, str]]]
    keep_outputs_on_error: bool
    echo: Optional[IO[bytes]]
    strip_dwo_members: Optional[str]
    archiver: Optional[list[str]]
    archiver_deterministic: bool
    rustc: list[str]


def arg_parse(argv: Optional[list[str]] = None) -> Args:
    # Command line is <action.py> [args] -- rustc command line
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument(
        "--diag-json",
        type=argparse.FileType("wb"),
        help="Json-formatted diagnostic output "
        "(assumes compiler is invoked with --error-format=json)",
    )
    parser.add_argument(
        "--diag-txt",
        type=argparse.FileType("wb"),
        help="Rendered text diagnostic output (also streamed to stderr)",
    )
    parser.add_argument(
        "--passthrough-stderr",
        action="store_true",
        help="Copy stderr through verbatim without diagnostic parsing "
        "(for commands whose output is not rustc JSON diagnostics)",
    )
    parser.add_argument(
        "--env",
        action="append",
        type=plain_key_value_arg,
        metavar="NAME=VALUE",
        help="Set environment",
    )
    parser.add_argument(
        "--path-env",
        action="append",
        type=key_value_arg,
        metavar="NAME=PATH",
        help="Set path environment (to be made absolute)",
    )
    parser.add_argument(
        "--remap-cwd-prefix",
        help="Remap paths under the current working directory to this path prefix",
    )
    parser.add_argument(
        "--crate-map",
        action="append",
        type=key_value_arg,
        metavar="CRATE=TARGET",
        help="Crate name to target map for unused crate diagnostics",
    )
    parser.add_argument(
        "--buck-target",
        help="Buck target for crate, used for unused crate diagnostics",
    )
    parser.add_argument(
        "--failure-filter",
        type=argparse.FileType("wb"),
        help="Consider a failure as success so long as we got some usable diagnostics",
        metavar="build-status.json",
    )
    parser.add_argument(
        "--required-output",
        action="append",
        nargs=2,
        metavar=("SHORT", "PATH"),
        help="Required output path we expect rustc to generate "
        "(and filled with a placeholder on a filtered failure)",
    )
    parser.add_argument(
        "--keep-outputs-on-error",
        action="store_true",
        help="Count the required outputs rustc writes even when it reports errors "
        "(an expansion is usable even if the expanded crate does not compile)",
    )
    parser.add_argument(
        "--echo",
        type=argparse.FileType("wb"),
        help="Write the input command line to this file, without running it",
    )
    parser.add_argument(
        "--strip-dwo-members",
        metavar="ARCHIVE",
        help="Archive (rlib or staticlib) written by the compiler from which "
        "to delete every `.dwo` member after a successful compile",
    )
    parser.add_argument(
        "--archiver",
        action="append",
        metavar="ARG",
        help="Archiver command used by --strip-dwo-members, one argument per occurrence",
    )
    parser.add_argument(
        "--archiver-deterministic",
        action="store_true",
        help="Pass the `D` modifier to the archiver so the rewritten archive carries no timestamps",
    )
    parser.add_argument(
        "rustc",
        nargs=argparse.REMAINDER,
        help="Compiler command line",
    )

    args = Args(**vars(parser.parse_args(argv)))
    if args.strip_dwo_members is not None and not args.archiver:
        parser.error("--strip-dwo-members requires --archiver")
    return args


def arg_eval(arg: str) -> str:
    """
    Expand the following two special cases:
        --extern=$(cat buck-out/v2/gen/foo.txt)=buck-out/dev/gen/libfoo.rlib
        --env-set=FOO=$(abspath buck-out/v2/gen/foo.txt)
    """
    expanded = ""

    while True:
        begin = arg.find("$(")
        if begin == -1:
            return expanded + arg
        expanded += arg[:begin]
        if arg[begin:].startswith("$(cat "):
            begin += len("$(cat ")
            path, rest = arg[begin:].split(")", maxsplit=1)
            with open(path, encoding="utf-8") as f:
                expanded += f.read().strip()
            arg = rest
        elif arg[begin:].startswith("$(abspath "):
            begin += len("$(abspath ")
            path, rest = arg[begin:].split(")", maxsplit=1)
            expanded += os.path.abspath(path)
            arg = rest
        else:
            expanded += "$("
            arg = arg[begin + len("$(") :]


def inherited_env() -> dict[str, str]:
    env = {}
    for pattern in INHERITED_ENV:
        if pattern.endswith("*"):
            for k in os.environ:
                if k.startswith(pattern[:-1]):
                    env[k] = os.environ[k]
        elif pattern in os.environ:
            env[pattern] = os.environ[pattern]
    return env


# The archiver's `d` operation names each member to delete and rewrites the
# whole archive, so all members should go in one invocation. Keep each
# invocation's argv under the smallest command-line limit of the hosts the
# rules run on (Windows: 32767 characters).
ARCHIVER_ARGV_BUDGET = 30_000


def strip_dwo_members(
    archiver: list[str], archive: str, deterministic: bool, env: dict[str, str]
) -> int:
    """Delete the `.dwo` members the compiler packed into an rlib or staticlib.

    Under `-Csplit-debuginfo=unpacked`, rustc writes each codegen unit's `.dwo`
    to `--out-dir` and also copies it into the archive, so that a downstream
    `-Csplit-debuginfo=packed` link could build a dwp from the archive alone.
    The rules never link that way: `dwp` reads the `--out-dir` files, and
    linkers never pull archive members that define no symbols. The copies are
    dead weight for every consumer of the archive. The archive is left
    untouched when it has no `.dwo` members.
    """
    # Apple's archiver takes its deterministic mode from the environment.
    env = {**env, "ZERO_AR_DATE": "1"}

    def members() -> list[str]:
        listing = subprocess.run(
            [*archiver, "t", archive], stdout=subprocess.PIPE, env=env, check=True
        )
        return [m for m in listing.stdout.decode().splitlines() if m.endswith(".dwo")]

    try:
        dwo_members = members()
        if not dwo_members:
            return 0

        # GNU-style archivers delete one member per name given, so a name that
        # occurs twice must be given twice. rustc never produces duplicates
        # within one archive; the check below catches it if that changes.
        delete = [*archiver, "dD" if deterministic else "d", archive]
        budget = ARCHIVER_ARGV_BUDGET - sum(len(arg) + 1 for arg in delete)
        chunks: list[list[str]] = [[]]
        used = 0
        for member in dwo_members:
            if chunks[-1] and used + len(member) + 1 > budget:
                chunks.append([])
                used = 0
            chunks[-1].append(member)
            used += len(member) + 1
        for chunk in chunks:
            subprocess.run([*delete, *chunk], env=env, check=True)

        remaining = members()
        if remaining:
            eprint(f"{archive} still has .dwo members after deleting them: {remaining}")
            return 1
    except subprocess.CalledProcessError as e:
        eprint(f"failed to strip .dwo members from {archive}: {e}")
        return e.returncode or 1

    return 0


async def handle_output(  # noqa: C901
    proc: asyncio.subprocess.Process,
    args: Args,
    crate_map: dict[str, str],
) -> bool:
    got_error_diag = False

    proc_stderr = proc.stderr
    assert proc_stderr is not None

    while True:
        line = await proc_stderr.readline()

        if line is None or line == b"":
            break

        if args.passthrough_stderr:
            sys.stderr.buffer.write(line)
            continue

        try:
            diag = json.loads(line)
        except json.JSONDecodeError:
            sys.stderr.buffer.write(line + b"\n")  # Passthrough
            continue

        if DEBUG:
            print(f"diag={repr(diag)}", end="\n")

        if "unused_extern_names" in diag:
            unused_names = diag["unused_extern_names"]

            # Empty unused_extern_names is just noise.
            # This happens when there are no unused crates.
            if not unused_names:
                continue

            # Treat error-level unused dep warnings as errors
            if diag.get("lint_level") in ("deny", "forbid"):
                got_error_diag = True

            # Add more information to unused crate warnings.
            if args.buck_target:
                rendered_unused = []
                for name in unused_names:
                    if name in crate_map:
                        rendered_unused.append(f"{crate_map[name]}: {name}")
                    else:
                        rendered_unused.append(f"{name}")
                rendered_unused.sort()
                rendered_unused = "\n    ".join(rendered_unused)

                diag["buck_target"] = args.buck_target
                diag["rendered"] = (
                    f"Target `{args.buck_target}` has unused dependencies:\n"
                    f"    {rendered_unused}"
                )
            diag["unused_deps"] = {
                name: crate_map[name] for name in unused_names if name in crate_map
            }
        else:
            if diag.get("level") == "error":
                got_error_diag = True

        # Emit json
        if args.diag_json:
            args.diag_json.write(
                json.dumps(diag, separators=(",", ":")).encode() + b"\n"
            )

        # Emit rendered text version
        if "rendered" in diag:
            rendered = diag["rendered"].encode() + b"\n"
            if args.diag_txt:
                args.diag_txt.write(rendered)
            sys.stderr.buffer.write(rendered)

    if args.diag_json:
        args.diag_json.close()
    if args.diag_txt:
        args.diag_txt.close()

    return got_error_diag


async def main() -> int:  # noqa: C901
    # `--program-args` is a raw argv boundary, split off before argparse's
    # response-file expansion so the tail is never inspected or rewritten.
    # First raw occurrence wins; absence (None) means ordinary behavior,
    # while an empty tail still emits a literal `--`. No wrapper option
    # takes a separate value that could equal this token.
    raw_argv = sys.argv[1:]
    program_args = None
    if "--program-args" in raw_argv:
        split = raw_argv.index("--program-args")
        program_args = raw_argv[split + 1 :]
        raw_argv = raw_argv[:split]
    args = arg_parse(raw_argv)

    if program_args is None:
        # No boundary: historical behavior; expand everything.
        positional = [arg_eval(arg) for arg in args.rustc]
    else:
        # With a boundary only compiler arguments expand; the prefix stays
        # raw apart from OUT_DIR resolution below.
        positional = list(args.rustc)
    args = args._replace(rustc=positional)

    if args.echo:
        echo_argv = list(args.rustc)
        if program_args is not None:
            echo_argv += ["--program-args"] + program_args
        args.echo.write("".join(arg + "\n" for arg in echo_argv).encode("utf-8"))
        return 0

    # Inherit a very limited initial environment, then add the new things
    env = inherited_env()
    if args.env:
        env.update({k: arg_eval(unescape_env_value(v)) for k, v in args.env})
    if args.path_env:
        env.update({k: os.path.abspath(v) for k, v in args.path_env})

    crate_map = dict(args.crate_map) if args.crate_map else {}

    if DEBUG:
        print(f"args {repr(args)} env {env} crate_map {crate_map}", end="\n")

    out_dir = env.get("OUT_DIR")

    separator = args.rustc.index("--rustc-action-separator")
    rustc_cmd, rustc_args_orig = args.rustc[:separator], args.rustc[separator + 1 :]
    if program_args is not None:
        # Expansion applies to compiler arguments only.
        rustc_args_orig = [arg_eval(arg) for arg in rustc_args_orig]
        if out_dir is not None:
            rustc_cmd = [
                arg.replace(OUT_DIR_SENTINEL, out_dir)
                if OUT_DIR_SENTINEL in arg
                else arg
                for arg in rustc_cmd
            ]

    rustc_args = []
    for arg in rustc_args_orig:
        # Resolve OUT_DIR_SENTINEL to the consumer's canonical OUT_DIR.
        if out_dir is not None and OUT_DIR_SENTINEL in arg:
            arg = arg.replace(OUT_DIR_SENTINEL, out_dir)
        # Build.bzl uses the following expression to generate remap flags:
        #   cmd_args("--remap-path-prefix=", ... "=", ctx.label.path, path_sep, delimiter = "")
        # The ctx.label.path (which is of type StarlarkCellPath) has the
        # inconvenient behavior that if the label's package has fewer than two
        # components, it gets an extra "./" prepended. So for targets //:repro
        # and //one:repro and //one/two:repro we would get remap flags with the
        # right-hand side as "./" and "./one/" and "one/two/". In compiler
        # diagnostics we would never want this leading "./" so strip it off.
        if arg.startswith("--remap-path-prefix="):
            flag, buck_out, mapped = arg.split("=", 2)
            if mapped.startswith("./"):
                mapped = mapped[2:]
            arg = f"{flag}={buck_out}={mapped}"

        # While the env-set feature is unstable, allow it to be used with stable
        # rustc by translating from a rustc flag to environment variables set
        # during the execution of the rustc subprocess. Our handling of Cargo
        # build scripts relies on `--env-set` as the implementation of "cargo:rustc-env".
        # Tracking issue: https://github.com/rust-lang/rust/issues/118372
        if arg.startswith("--env-set="):
            flag, key, value = arg.split("=", 2)
            env[key] = value
            continue

        rustc_args.append(arg)

    if args.remap_cwd_prefix is not None:
        rustc_args.append(f"--remap-path-prefix={os.getcwd()}={args.remap_cwd_prefix}")
        rustc_args.append(
            "--remap-path-prefix={}={}".format(
                os.path.realpath(os.getcwd()), args.remap_cwd_prefix
            )
        )

    # The tail travels as raw argv past the response file; Miri scans raw
    # argv itself, so the separator must not hide inside the file.
    prog_tail_argv = []
    if program_args is not None:
        prog_tail_argv = ["--", *program_args]

    if args.failure_filter and args.keep_outputs_on_error:
        for _short, path in args.required_output or []:
            if Path(path).exists():
                Path(path).unlink()

    with tempfile.NamedTemporaryFile(
        mode="wb",
        prefix="rustc-args-",
        suffix=".txt",
        delete=False,
        # This isn't set when running doctests. Once that's fixed, we won't need
        # `tempfile`
        dir=os.environ.get("BUCK_SCRATCH_PATH", None),
    ) as args_file:
        args_file.write("\n".join(rustc_args).encode() + b"\n")
        args_file.flush()
        args_file.close()
        # Kick off the action
        proc = await asyncio.create_subprocess_exec(
            *rustc_cmd,
            "@" + args_file.name,
            *prog_tail_argv,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=None,  # Inherit
            stderr=subprocess.PIPE,
            limit=20_000_000,
        )
        got_error_diag = await handle_output(proc, args, crate_map)
        res = await proc.wait()

        # TODO: When Python 3.12 becomes the baseline, replace this with:
        #   `NamedTemporaryFile(delete=True, delete_on_close=False)`
        os.unlink(args_file.name)

    if DEBUG:
        print(
            f"res={repr(res)} "
            f"got_error_diag={got_error_diag} "
            f"args.failure_filter {args.failure_filter}",
            end="\n",
        )

    # If rustc is reporting a silent error, make it loud
    if res == 0 and got_error_diag:
        res = 1

    if res == 0 and args.strip_dwo_members is not None:
        assert args.archiver is not None
        res = strip_dwo_members(
            args.archiver, args.strip_dwo_members, args.archiver_deterministic, env
        )

    # Check for death by signal - this is always considered a failure
    if res < 0:
        cmdline = shlex.join(rustc_cmd + rustc_args)
        eprint(f"Command exited with signal {-res}: command line: {cmdline}")
    elif args.failure_filter:
        # If failure filtering is enabled, then getting an error diagnostic is also
        # considered a success. That is, if rustc exited with an error status, but
        # we saw a fatal error diagnostic, we can still report a zero exit status.
        # This still means we had an error if someone wanted one of the other output
        # artifacts as an input, but the failure filter action will handle that by
        # examining the build_status json output.

        required_output = args.required_output or []

        # We need to create a build status output, including the list of output
        # files which were *actuallyq* created. We use the short paths as the
        # logical filename rather than the actual full path, since that won't
        # mean much to a downstream action.
        # An output at its path is only this run's when rustc succeeded: an action that keeps
        # its outputs (incremental compiles) starts with the previous run's, and a filtered
        # failure leaves a placeholder behind. `--keep-outputs-on-error` deletes them first.
        build_status = {
            "status": res,
            "files": [
                short
                for short, path in required_output
                if (res == 0 or args.keep_outputs_on_error) and Path(path).exists()
            ],
        }
        args.failure_filter.write(
            json.dumps(build_status, separators=(",", ":")).encode() + b"\n"
        )

        # OK to actually report success, but keep buck happy by making sure all
        # the required outputs are present
        if got_error_diag and res != 0:
            for _short, path in required_output:
                path = Path(path)
                if not path.exists():
                    path.touch()
            res = 0

    return res


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
