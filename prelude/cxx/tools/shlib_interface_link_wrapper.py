#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Runs a link command, then derives a shared library interface from the linked
output with the toolchain's interface producer, so both come out of one action.

Usage: <linked output> <interface output> <producer command...> <separator> <link command...>

The separator is a token no producer or linker uses as an argument, since a
plain `--` can appear in either command.
"""

import subprocess
import sys

SEPARATOR = "--shlib-interface-link-wrapper-link-command"


def main(argv: list[str]) -> int:
    linked_output, interface_output, rest = argv[0], argv[1], argv[2:]
    separator = rest.index(SEPARATOR)
    producer, link_command = rest[:separator], rest[separator + 1 :]

    returncode = subprocess.call(link_command)
    if returncode != 0:
        return returncode

    return subprocess.call(producer + [linked_output, interface_output])


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
