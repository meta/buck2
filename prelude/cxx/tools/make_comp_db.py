#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Utility to create compilation DBs

$ make_comp_db.py gen --output=entry.json foo.cpp -- g++ -c -fPIC
$ make_comp_db.py gen --output=entry2.json foo2.cpp -- g++ -c -fPIC
$ make_comp_db.py merge --output=comp_db.json entry.json entry2.json
"""

import argparse
import json
import shlex
import sys
from typing import List


def process_arguments(arguments: List[str]) -> List[str]:
    """
    Process arguments to expand argsfiles.
    """

    combined_arguments = []
    for arg in arguments:
        if arg.startswith("@"):
            with open(arg[1:]) as argsfile:
                # The argsfile's arguments are separated by newlines; we
                # don't want those included in the argument list.
                lines = [" ".join(shlex.split(line)) for line in argsfile.readlines()]
                # Support nested argsfiles.
                combined_arguments.extend(process_arguments(lines))
        else:
            combined_arguments.append(arg)
    return combined_arguments


def gen(args: argparse.Namespace) -> None:
    """
    Generate a single compilation command in JSON form.
    """

    entry = {}
    entry["file"] = args.directory + "/" + args.filename
    entry["directory"] = "."
    entry["arguments"] = process_arguments(args.arguments)

    json.dump(entry, args.output, indent=2)
    args.output.close()


def merge(args: argparse.Namespace) -> None:
    """
    Merge multiple compilation DB commands into a single DB.
    """

    entries = []
    for entry in args.entries:
        if entry.startswith("@"):
            with open(entry[1:]) as argsfile:
                sub_entries = map(str.strip, argsfile.readlines())

                for sub_entry in sub_entries:
                    with open(sub_entry) as g:
                        entries.append(json.load(g))
        else:
            with open(entry) as f:
                entries.append(json.load(f))

    json.dump(entries, args.output, indent=2)
    args.output.close()


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers()

    parser_gen = subparsers.add_parser("gen")
    parser_gen.add_argument("--output", type=argparse.FileType("w"), default=sys.stdout)
    parser_gen.add_argument("filename")
    parser_gen.add_argument("directory")
    parser_gen.add_argument("arguments", nargs="*")
    parser_gen.set_defaults(func=gen)

    parser_merge = subparsers.add_parser("merge")
    parser_merge.add_argument(
        "--output", type=argparse.FileType("w"), default=sys.stdout
    )
    parser_merge.add_argument("entries", nargs="*")
    parser_merge.set_defaults(func=merge)

    args = parser.parse_args(argv[1:])
    args.func(args)
    return 0


sys.exit(main(sys.argv))
