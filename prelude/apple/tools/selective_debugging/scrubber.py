# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import json
import os
import shutil
import subprocess
from collections.abc import Callable
from typing import BinaryIO, NamedTuple, Optional

from apple.tools.re_compatibility_utils.writable import make_path_user_writable

from .macho_parser import load_commands, load_debug_symbols, load_header
from .spec import Spec
from .utils import MachOException


FAKE_PATH = b"fake/path"
# Covers almost every path in one read: PATH_MAX is 1024 on macOS.
_PATH_READ_SIZE = 1024
# buck-out/isolation_dir/gen/project_cell/{hash}/....
NUM_OF_COMPONENTS_IN_BUCK2_OUTPUT_PATH_BEFORE_PROJECT_PATH = 5
# buck-out/isolation_dir/gen/project_cell//X/Y/__name__/{hash}/....
NUM_OF_COMPONENTS_IN_BUCK2_OUTPUT_PATH_BEFORE_PROJECT_PATH_WITH_CONTENT_BASED_PATH = 4


def _always_scrub(_: str) -> bool:
    return True


# Visible for testing
def load_focused_targets_output_paths(json_file_path: str) -> set[str]:
    if json_file_path is None or not os.path.exists(json_file_path):
        return set()
    with open(json_file_path) as f:
        content = f.read()
        if not content:
            return set()
        data = json.loads(content)
        output_paths = set()
        for target in data["targets"]:
            _, package_and_name = target.split("//")
            package, name = package_and_name.split(":")
            # This assumes the output path created by buck2, which if
            # modified, would break this logic.
            output_directory = f"{package}/__{name}__"
            output_paths.add(output_directory)
    return output_paths


# This function converts buck-out/isolation_dir/gen/project_cell/{hash}/X/Y/__name__/libFoo.a
# into X/Y/__name__ to match the focus target output paths created by load_focused_targets_output_paths.
# Rules can nest outputs in their own double underscore directories, so every such
# directory yields a candidate, innermost first.
# Visible for testing
def _get_target_output_paths_from_debug_file_path(
    debug_target_path: str,
) -> list[str]:
    # This function assumes the debug file path created by buck2 in one of the following formats:
    # Without content based path:
    # buck-out/isolation_dir/gen/project_cell/{hash}/.../__name__/libFoo.a
    # buck-out/isolation_dir/gen/project_cell/{hash}/.../__name__/__objects__/bar.o
    # buck-out/isolation_dir/gen/project_cell/{hash}/.../__name__/swift_object_file.swift.o
    # buck-out/isolation_dir/gen/project_cell/{hash}/.../__name__/__swift_incremental__/objects/bar.swift.o
    # With content based path:
    # buck-out/isolation_dir/gen/project_cell/.../__name__/{hash}/libFoo.a
    # buck-out/isolation_dir/gen/project_cell/.../__name__/__objects__/{hash}/bar.o
    # buck-out/isolation_dir/gen/project_cell/.../__name__/{hash}/swift_object_file.swift.o
    parts = debug_target_path.split("/")

    candidates = []
    for i in range(len(parts) - 1, -1, -1):
        if not (parts[i].startswith("__") and parts[i].endswith("__")):
            continue
        # This handles the two cases, one without content based path, and one with
        for prefix_len in (
            NUM_OF_COMPONENTS_IN_BUCK2_OUTPUT_PATH_BEFORE_PROJECT_PATH,
            NUM_OF_COMPONENTS_IN_BUCK2_OUTPUT_PATH_BEFORE_PROJECT_PATH_WITH_CONTENT_BASED_PATH,
        ):
            # A directory inside the buck-out prefix cannot be a target output directory
            if i >= prefix_len:
                candidates.append("/".join(parts[prefix_len : i + 1]))
    if not candidates:
        raise Exception(
            f"Unrecognized format for debug file path : {debug_target_path}"
        )

    return candidates


def get_target_output_paths(debug_file_path: str) -> list[str]:
    """
    Returns the output directories (`package/__name__`, as returned by
    `load_focused_targets_output_paths()`) of the targets which may have produced
    the object file or archive of an `N_OSO` entry.
    """
    # debug_file_path usually have the format x/y/z/libFoo.a(bar.m.o)
    if debug_file_path.startswith("buck-out/"):
        return _get_target_output_paths_from_debug_file_path(
            debug_file_path.split("(")[0]
        )

    # occasionally archive file can be directly from source.
    candidates = []
    (package, name) = os.path.split(debug_file_path)
    while package != "" and package != "/":
        candidates.append(f"{package}/__{name}__")
        (package, name) = os.path.split(package)
    return candidates


# Visible for testing
def should_scrub_with_focused_targets_output_paths(
    focused_targets_output_paths: set[str], debug_file_path: str
) -> bool:
    # All paths to be scrubbed when no focused target is specified
    if len(focused_targets_output_paths) == 0:
        return True

    return not any(
        target_output_path in focused_targets_output_paths
        for target_output_path in get_target_output_paths(debug_file_path)
    )


def _should_scrub_with_targets_file(
    json_file_path: str, additional_labels: set[str]
) -> Callable[[str], bool]:
    focused_targets_output_paths = load_focused_targets_output_paths(
        json_file_path
    ).union(additional_labels)
    return lambda debug_file_path: should_scrub_with_focused_targets_output_paths(
        focused_targets_output_paths, debug_file_path
    )


def _should_scrub_with_spec_file(
    json_file_path: str, additional_labels: set[str]
) -> Callable[[str], bool]:
    spec = Spec(json_file_path)
    return lambda debug_file_path: should_scrub_with_focused_targets_output_paths(
        additional_labels, debug_file_path
    ) and spec.scrub_debug_file_path(debug_file_path)


class DebugFilePath(NamedTuple):
    """
    The path of an `N_OSO` entry, the file offset of its string and the
    string's length in bytes, without the NUL.
    """

    offset: int
    size: int
    path: str


def _scrub(
    f: BinaryIO,
    debug_file_paths: list[DebugFilePath],
    scrub_handler: Callable[[str], bool],
) -> list[tuple[str, str]]:
    """
    Return a list of tuples.
    Each tuple contains a pair of the original path and the rewritten path
    """
    results = []
    for offset, size, path_str in debug_file_paths:
        if scrub_handler(path_str):
            f.seek(offset)
            # We don't want to modify the length of the path, so pad the replacement
            # path with spaces
            buffer = FAKE_PATH + b" " * (size - len(FAKE_PATH))
            f.write(buffer)
            results.append((path_str, buffer.decode()))
        else:
            results.append((path_str, path_str))
    return results


def make_scrub_handler(
    persisted_targets_file: Optional[str],
    targets_file: Optional[str] = None,
    spec_file: Optional[str] = None,
) -> Callable[[str], bool]:
    """
    Returns a function which tells whether the debug file path of an `N_OSO`
    entry should be scrubbed, i.e. whether its debug info should be excluded.
    """
    additional_labels = load_focused_targets_output_paths(persisted_targets_file)
    if targets_file and spec_file:
        raise Exception(
            "Only one of a targets file or spec file is supported, not both!"
        )
    elif targets_file:
        return _should_scrub_with_targets_file(targets_file, additional_labels)
    elif spec_file:
        return _should_scrub_with_spec_file(spec_file, additional_labels)
    else:
        return _always_scrub


def load_debug_file_paths(f: BinaryIO) -> list[DebugFilePath] | None:
    """
    Returns the debug file paths of the `N_OSO` entries of a Mach-O file, with
    where each path is stored. Returns `None` if the file has no symbol table to
    look at.
    """
    header, offset = load_header(f, 0)
    if not header.is_valid:
        raise MachOException("Invalid macho format!")
    lc_linkedit, lc_symtab = load_commands(f, offset, header.n_cmds)
    if lc_linkedit is None:
        return None
    if lc_symtab is None:
        raise MachOException("LC_SYMTAB command not found")
    if lc_symtab.strtab_size == 0 or lc_symtab.n_symbols == 0:
        return None
    if lc_linkedit.file_size == 0:
        raise MachOException("LC_SEGMENT_64 command for string table not found")
    f.seek(lc_symtab.strtab_offset)
    """
    ld64 deliberately burns the first byte with the space character, so that zero is never a
    valid string index and writes 0x00 at offset 1, so that it's always the empty string.
    The code for this in ld64 is in LinkEditClassic.hpp (StringPoolAtom::StringPoolAtom).
    """
    if f.read(1) != b"\x20":
        raise MachOException("First character in the string table is not a space")
    if f.read(1) != b"\x00":
        raise MachOException("Second character in the string table is not a NUL")

    symbols = load_debug_symbols(f, lc_symtab.symtab_offset, lc_symtab.n_symbols)
    debug_file_paths = []
    for symbol in symbols:
        start = lc_symtab.strtab_offset + symbol.strtab_index
        f.seek(start)

        # Read until we reach the end of the path, denoted by Hex 0.
        path = b""
        end = -1
        while end == -1:
            chunk = f.read(_PATH_READ_SIZE)
            if not chunk:
                raise MachOException("Unterminated string in the string table")
            end = chunk.find(b"\x00")
            path += chunk if end == -1 else chunk[:end]
        debug_file_paths.append(DebugFilePath(start, len(path), path.decode()))
    return debug_file_paths


def scrub(
    input_file: str,
    output_file: str,
    persisted_targets_file: str,
    targets_file: Optional[str] = None,
    spec_file: Optional[str] = None,
    adhoc_codesign_tool: Optional[str] = None,
) -> list[tuple[str, str]]:
    scrub_handler = make_scrub_handler(persisted_targets_file, targets_file, spec_file)

    shutil.copy2(input_file, output_file)
    # Make it RE-compatible
    make_path_user_writable(output_file)

    results = []
    with open(output_file, "r+b") as f:
        debug_file_paths = load_debug_file_paths(f)
        if debug_file_paths is None:
            return []
        results = _scrub(f, debug_file_paths, scrub_handler)

    if adhoc_codesign_tool:
        subprocess.run(
            [adhoc_codesign_tool, "--binary", output_file],
            check=True,
        )

    return results
