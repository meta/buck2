# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import argparse
import os
import shlex
import subprocess
import sys
from pathlib import Path


# A copy of "cmd/internal/quoted" translated into Python with GPT-4
# Source: https://github.com/golang/go/blob/7e9894449e8a12157a28a4a14fc9341353a6469c/src/cmd/internal/quoted/quoted.go#L65
def go_join(args):
    buf = []
    for i, arg in enumerate(args):
        if i > 0:
            buf.append(" ")
        saw_space, saw_single_quote, saw_double_quote = False, False, False
        for c in arg:
            if ord(c) > 127:
                continue
            elif c.isspace():
                saw_space = True
            elif c == "'":
                saw_single_quote = True
            elif c == '"':
                saw_double_quote = True
        if not saw_space and not saw_single_quote and not saw_double_quote:
            buf.append(arg)
        elif not saw_single_quote:
            buf.append("'")
            buf.append(arg)
            buf.append("'")
        elif not saw_double_quote:
            buf.append('"')
            buf.append(arg)
            buf.append('"')
        else:
            raise ValueError(
                f"Argument {arg} contains both single and double quotes and cannot be quoted"
            )
    return "".join(buf)


def main(argv):
    """
    This is a wrapper script around the `go` binary.
    - It fixes GOROOT and GOCACHE
    """
    if len(argv) < 2:
        print("usage: go_wrapper.py <wrapped binary> <args>", file=sys.stderr)
        return 1

    wrapped_binary = Path(argv[1]).resolve()

    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument("--workdir", type=Path, default=None)
    parser.add_argument("--output", type=argparse.FileType("w"), default=sys.stdout)
    parsed, unknown = parser.parse_known_args(argv[2:])

    env = os.environ.copy()
    # Make paths absolute, otherwise go build will fail.
    if "GOROOT" in env:
        env["GOROOT"] = os.path.realpath(env["GOROOT"])

    env["GOCACHE"] = os.path.realpath(env["BUCK_SCRATCH_PATH"])

    cwd = os.getcwd()
    for env_var in ["CC", "CGO_CFLAGS", "CGO_CPPFLAGS", "CGO_LDFLAGS"]:
        if env_var in env:
            # HACK: Split the value into a list of arguments then join them back.
            # This is because buck encodes quoted args in a way `go` doesn't like,
            # but `go_join` does it in a way that `go` expects.
            var_value = go_join(shlex.split(env[env_var]))
            # HACK: Replace %cwd% with the current working directory to make it work when `go` does `cd` to a tmp-dir.
            env[env_var] = var_value.replace("%cwd%", cwd)

    unknown = [arg.replace("%cwd%", cwd) for arg in unknown]

    retcode = subprocess.call(
        [wrapped_binary] + unknown, env=env, cwd=parsed.workdir, stdout=parsed.output
    )
    parsed.output.close()
    return retcode


sys.exit(main(sys.argv))
