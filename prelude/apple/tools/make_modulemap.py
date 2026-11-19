#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import argparse
import os
import re
from io import TextIOWrapper


_RESERVED_KEYWORDS: frozenset[str] = frozenset(
    [
        "config_macros",
        "conflict",
        "exclude",
        "explicit",
        "extern",
        "export_as",
        "export",
        "framework",
        "header",
        "link",
        "module",
        "private",
        "requires",
        "textual",
        "umbrella",
        "use",
    ]
)


class Module:
    def __init__(self, name: str, is_framework: bool) -> None:
        self.name: str = name
        self.headers: list[str] = []
        self.submodules: dict[str, Module] = {}
        self.is_framework: bool = is_framework

    def add_header(self, src: str) -> None:
        self.headers.append(src)

    def get_submodule(self, name: str, is_framework: bool) -> "Module":
        if name not in self.submodules:
            self.submodules[name] = Module(name, is_framework)

        return self.submodules[name]

    def render(self, f: TextIOWrapper, indent: int = 0) -> None:
        space = " " * indent
        name = self.name
        if name in _RESERVED_KEYWORDS:
            name = f"{name}_"
        if self.is_framework:
            f.write(f"{space}framework module {name} {{\n")
        else:
            f.write(f"{space}module {name} {{\n")

        submodule_names = set()
        for submodule_name in sorted(self.submodules.keys()):
            submodule = self.submodules[submodule_name]

            # remove any extensions for readability
            sanitized_name = os.path.splitext(submodule_name)[0]

            # module names can only be ascii or _
            sanitized_name = re.sub(r"[^A-Za-z0-9_]", "_", sanitized_name)
            if sanitized_name[0].isdigit():
                sanitized_name = "_" + sanitized_name

            # a reserved keyword is renamed, so collisions must be checked against that name
            if sanitized_name in _RESERVED_KEYWORDS:
                sanitized_name = f"{sanitized_name}_"

            # avoid any collisions with other files
            while sanitized_name in submodule_names:
                sanitized_name += "_"

            submodule_names.add(sanitized_name)
            submodule.name = sanitized_name
            submodule.render(f, indent + 4)

        header_space = " " * (indent + 4)
        for h in sorted(self.headers):
            f.write(f'{header_space}header "{h}"\n')

        if self.headers:
            f.write(f"{header_space}export *\n")

        f.write(f"{space}}}\n")


def _write_single_module(
    f: TextIOWrapper,
    name: str,
    headers: dict[str, str],
    is_framework: bool,
) -> None:
    module = Module(name, is_framework)
    for path in headers.values():
        module.add_header(path)

    module.render(f)


def _write_submodules(
    f: TextIOWrapper,
    name: str,
    headers: dict[str, str],
    is_framework: bool,
) -> None:
    # Create a tree of nested modules, one for each path component.
    root_module = Module(name, is_framework)
    for include_path, disk_path in headers.items():
        module = root_module
        for i, component in enumerate(include_path.split(os.sep)):
            if i == 0 and component == name:
                # The common case is we have a single header path prefix that matches the module name.
                # In this case we add the headers directly to the root module.
                pass
            else:
                module = module.get_submodule(component, is_framework)

        module.add_header(disk_path)

    root_module.render(f)


def _write_swift_header(f: TextIOWrapper, name: str, swift_header_path: str) -> None:
    f.write(
        f"""
module {name}.Swift {{
    header "{swift_header_path}"
    requires objc
}}
"""
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output", required=True, help="The path to write the modulemap to"
    )
    parser.add_argument("--name", required=True, help="The name of the module")
    parser.add_argument(
        "--swift-header", help="If this is a mixed module extend with this Swift header"
    )
    parser.add_argument(
        "--use-submodules",
        action="store_true",
        help="If set produce a modulemap with per-header submodules",
    )
    parser.add_argument(
        "--framework",
        help="This modulemap is for embedding in a framework.",
        action="store_true",
    )
    parser.add_argument(
        "mappings",
        nargs="*",
        default=[],
        help="A list of tuples of include path to disk paths",
    )
    args = parser.parse_args()

    if len(args.mappings) % 2 != 0:
        raise RuntimeError(
            "Mappings must be a list of pairs of include path to disk path"
        )

    headers = {}
    for i in range(0, len(args.mappings), 2):
        path = args.mappings[i + 1]
        if args.framework:
            pass
        else:
            # We need to relativize the path to the modulemap file
            path = os.path.relpath(path, os.path.dirname(args.output))

        headers[args.mappings[i]] = path

    with open(args.output, "w") as f:
        if args.use_submodules:
            _write_submodules(
                f,
                args.name,
                headers,
                args.framework,
            )
        else:
            _write_single_module(
                f,
                args.name,
                headers,
                args.framework,
            )

        if args.swift_header:
            _write_swift_header(
                f,
                args.name,
                os.path.relpath(args.swift_header, os.path.dirname(args.output)),
            )


if __name__ == "__main__":
    main()
