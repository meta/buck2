#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import subprocess
import sys

import dep_file_utils


def rewrite_dep_file(src_path, dst_path):
    """
    Convert a makefile to a depfile suitable for use by Buck2. The files we
    rewrite look like P488268797.
    """

    with open(src_path) as f:
        body = f.read()

    parts = body.split(": ", 1)
    body = parts[1] if len(parts) == 2 else ""

    # Escaped newlines are not meaningful so remove them.
    body = body.replace("\\\n", "")

    # Now, recover targets. They are separated by whitespace, and clang and gcc
    # escape the characters that make treats specially: a space as `\ ` (also
    # doubling the backslashes in front of it), `#` as `\#` and `$` as `$$`.
    # Any other backslash is part of the path.
    deps = []
    current = []
    pos = 0
    while pos < len(body):
        char = body[pos]
        if char == "\\":
            end = pos
            while end < len(body) and body[end] == "\\":
                end += 1
            backslashes = end - pos
            following = body[end : end + 1]
            if following == " ":
                current.append("\\" * (backslashes // 2))
                if backslashes % 2 == 1:
                    current.append(" ")
                    end += 1
            elif following == "#":
                current.append("\\" * (backslashes - 1) + "#")
                end += 1
            else:
                current.append("\\" * backslashes)
            pos = end
        elif char == "$" and body[pos + 1 : pos + 2] == "$":
            current.append("$")
            pos += 2
        elif char.isspace():
            if current:
                deps.append("".join(current))
                current = []
            pos += 1
        else:
            current.append(char)
            pos += 1
    if current:
        deps.append("".join(current))

    # Now that we've parsed deps, we need to normalize them.
    dep_file_utils.normalize_and_write_deps(deps, dst_path)


def process_dep_file(args):
    """
    Expects the src dep file to be the first argument, dst dep file to be the
    second argument, and the command to follow.
    """
    ret = subprocess.call(args[2:], stdin=subprocess.DEVNULL)
    if ret == 0:
        rewrite_dep_file(args[0], args[1])
    sys.exit(ret)
