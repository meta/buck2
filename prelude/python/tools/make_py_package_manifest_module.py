#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


"""
Generate a __manifest__.py module containing build metadata for a Python package.
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__,
        fromfile_prefix_chars="@",
    )
    parser.add_argument(
        "--module-manifests",
        help="A path to a list of JSON file with modules contained in the PEX.",
        type=Path,
        default=None,
    )
    parser.add_argument(
        "--manifest-entries",
        help="Path to a JSON file with build metadata entries.",
        type=Path,
        default=None,
    )
    parser.add_argument(
        "--manifest-entries-overlay",
        help="Path to JSON entries to merge into the base manifest entries.",
        type=Path,
        default=None,
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Output path for the generated module.",
        required=True,
    )
    parser.add_argument("--output-json", type=Path, default=None)
    return parser.parse_args()


# The ABI tag of an extension (`.cpython-312-x86_64-linux-gnu`, `.abi3`) is not
# part of its module name.
_SOABI_TAG = re.compile(r"\.(cpython-[^./\\]+|abi3)$")

# Marks an extension linked into the executable. It isn't an importable file,
# but its package is.
_STATIC_EXTENSION_STUB_SUFFIX = ".empty_stub"


def path_to_module(path: str) -> str | None:
    for suffix in (".py", ".so", ".pyd"):
        if path.endswith(suffix):
            stem = path[: -len(suffix)]
            if suffix != ".py":
                stem = _SOABI_TAG.sub("", stem)
            return stem.replace("/", ".").replace("\\", ".")


def merge_entries(entries: dict[str, object], overlay: dict[str, object]) -> None:
    for key, value in overlay.items():
        existing = entries.get(key)
        if isinstance(existing, dict) and isinstance(value, dict):
            existing.update(value)
        else:
            entries[key] = value


def load_entries(path: Path | None) -> dict[str, object]:
    if path is None:
        return {}
    with open(path) as f:
        entries = json.load(f)
    if not isinstance(entries, dict):
        raise ValueError(f"Manifest entries in {path} aren't a dictionary")
    return entries


def main() -> None:
    args = parse_args()
    output: Path = args.output
    if output.exists():
        raise ValueError(
            f"Output path '{output}' already exists, refusing to overwrite."
        )

    modules: dict[str, str] = {}

    with open(args.module_manifests) as me:
        module_manifests = me.read().splitlines()
        for module_manifest_file in module_manifests:
            with open(module_manifest_file) as f:
                for pkg_path, _, origin_desc in json.load(f):
                    module = path_to_module(pkg_path)
                    if module:
                        modules[module] = origin_desc
                    elif not pkg_path.endswith(_STATIC_EXTENSION_STUB_SUFFIX):
                        continue
                    # Add artificial __init__.py files like in make_py_package_modules.py
                    for parent in Path(pkg_path).parents:
                        if parent == Path("") or parent == Path("."):
                            continue
                        path = str(parent / "__init__.py")
                        parent_module = path_to_module(path)
                        if parent_module and parent_module not in modules:
                            modules[parent_module] = origin_desc
                        elif parent_module != module:
                            break

    entries = load_entries(args.manifest_entries)
    if args.manifest_entries_overlay:
        merge_entries(entries, load_entries(args.manifest_entries_overlay))
    if "modules" in entries:
        raise ValueError("'modules' can't be a key in manifest entries")
    if args.output_json:
        args.output_json.write_text(json.dumps(entries, sort_keys=True) + "\n")
    sorted_modules = sorted(modules.items())
    entries["modules"] = [m[0] for m in sorted_modules]
    entries["origins"] = tuple(m[1] for m in sorted_modules)
    output.write_text(
        "\n".join((f"{key} = {repr(value)}" for key, value in entries.items()))
    )


if __name__ == "__main__":
    main()
