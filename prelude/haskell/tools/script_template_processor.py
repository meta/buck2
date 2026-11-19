#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


"""
Helper script to generate scripts required by haskell tools from script
template files.

These script templates require GHC and/or toolchain values, e.g. GHC flags used,
paths to cxx tools.

The macros in the script should be in the format `<MACRO_NAME>`.
For example, `echo <binutils_path>` is an expression that would be replaced.
"""

import argparse
import os
import sys
from typing import Dict, List, Optional

# Paths in the toolchain that are passed relative to the project's cell and
# need to be canonicalized before being replaced in the script.
TOOLCHAIN_PATHS = [
    "binutils_path",
    "ghci_lib_path",
    "cc_path",
    "cpp_path",
    "cxx_path",
    "ghci_iserv_path",
    "ghc_pkg_path",
    "ghc_path",
]


def _replace_template_values(
    template_file,
    output_fpath: str,
    user_ghci_path: Optional[str],
    rel_toolchain_paths: Dict[str, str],
    exposed_packages: List[str],
    package_dbs: Optional[str],
    prebuilt_package_dbs: Optional[str],
    string_macros: Dict[str, Optional[str]],
):
    script_template = template_file.read()

    # Plain replacement throughout: a regex replacement would read backslash escapes in
    # the values (compiler flags, paths), and the values can hold any text.
    for macro_name, rel_path in rel_toolchain_paths.items():
        # Skip macro if its path wasn't provided for a specific template
        if not rel_path:
            continue

        # Follow symlink to get canonical path
        canonical_path = os.path.realpath(rel_path)

        script_template = script_template.replace(f"<{macro_name}>", canonical_path)

    if user_ghci_path is not None:
        # user_ghci_path has to be handled separately because it needs to be passed
        # with the ghci_lib_path as the `-B` argument.
        ghci_lib_path = rel_toolchain_paths["ghci_lib_path"]

        if ghci_lib_path:
            ghci_lib_canonical_path = os.path.realpath(ghci_lib_path)

            replacement = "${{DIR}}/{user_ghci_path} -B{ghci_lib_path}".format(
                user_ghci_path=user_ghci_path,
                ghci_lib_path=ghci_lib_canonical_path,
            )
        else:
            replacement = "${{DIR}}/{user_ghci_path}".format(
                user_ghci_path=user_ghci_path,
            )

        script_template = script_template.replace("<user_ghci_path>", replacement)

    if len(exposed_packages) > 0:
        script_template = script_template.replace(
            "<exposed_packages>", " ".join(exposed_packages)
        )

    prebuilt_package_dbs_args = None
    package_dbs_args = None

    if prebuilt_package_dbs is not None:
        prebuilt_pkgs_canonical_paths = [
            os.path.realpath(
                rel_path,
            )
            for rel_path in prebuilt_package_dbs.split(" ")
        ]

        prebuilt_package_dbs_args = " ".join(
            ["-package-db {}".format(pkg) for pkg in prebuilt_pkgs_canonical_paths]
        )

    if package_dbs is not None:
        package_dbs_args = " ".join(
            ["-package-db ${{DIR}}/{}".format(pkg) for pkg in package_dbs.split(" ")]
        )

    script_template = script_template.replace(
        "<package_dbs>",
        " ".join(filter(None, [prebuilt_package_dbs_args, package_dbs_args])),
    )

    # Replace all other macros
    for macro_name, macro_value in string_macros.items():
        if macro_value is not None:
            script_template = script_template.replace(f"<{macro_name}>", macro_value)

    with open(output_fpath, "w") as output_file:
        output_file.write(script_template)

    os.chmod(output_fpath, 0o777)  # Script should be executable


def main() -> int:
    parser = argparse.ArgumentParser()

    parser.add_argument(
        "--script_template",
        type=argparse.FileType("r"),
    )

    for tpath in TOOLCHAIN_PATHS:
        parser.add_argument(
            "--{}".format(tpath),
        )

    parser.add_argument(
        "--user_ghci_path",
    )

    parser.add_argument(
        "--exposed_packages",
        type=str,
        nargs="+",
    )
    parser.add_argument(
        "--output",
        help="The final script with all macros replaced.",
    )

    STRING_ARGS = [
        "package_dbs",
        "prebuilt_package_dbs",
        # Name of the buck target being built, e.g. haxlsh
        "target_name",
        "start_ghci",
        # Path to the final iserv script. Used by the final ghci script.
        "iserv_path",
        "compiler_flags",
        "srcs",
        "preload_libs",
        "squashed_so",
    ]

    for str_arg in STRING_ARGS:
        parser.add_argument(
            "--{}".format(str_arg),
            type=str,
        )

    args = parser.parse_args()

    rel_toolchain_paths = {}
    for tpath in TOOLCHAIN_PATHS:
        rel_toolchain_paths[tpath] = getattr(args, tpath)

    # Macros that don't require special handling and are not paths that need
    # to be canonicalized.
    string_macros = {
        "name": args.target_name,
        "start_ghci": args.start_ghci,
        "iserv_path": args.iserv_path,
        "compiler_flags": args.compiler_flags,
        "srcs": args.srcs,
        "squashed_so": args.squashed_so,
        "preload_libs": args.preload_libs,
    }

    _replace_template_values(
        args.script_template,
        args.output,
        args.user_ghci_path,
        rel_toolchain_paths,
        args.exposed_packages,
        args.package_dbs,
        args.prebuilt_package_dbs,
        string_macros,
    )

    return 0


if __name__ == "__main__":
    sys.exit(main())
