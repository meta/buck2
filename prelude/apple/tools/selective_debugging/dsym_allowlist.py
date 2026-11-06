# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import json

from .scrubber import get_target_output_paths, load_debug_file_paths, make_scrub_handler


def kept_debug_file_paths(
    input_file: str,
    persisted_targets_file: str | None,
    targets_file: str | None = None,
    spec_file: str | None = None,
) -> list[str]:
    """
    Returns the debug file paths of the `N_OSO` entries of `input_file` that
    the scrubber would keep, i.e. the object files (`foo.o`) and archive members
    (`libfoo.a(foo.o)`) whose debug info should end up in the dSYM. The file is
    only read.
    """
    should_scrub = make_scrub_handler(persisted_targets_file, targets_file, spec_file)
    with open(input_file, "rb") as f:
        debug_file_paths = load_debug_file_paths(f) or []
    return sorted({p.path for p in debug_file_paths if not should_scrub(p.path)})


def linked_target_output_paths(input_file: str) -> list[str]:
    """
    Returns the output directories (`package/__name__`) of the targets whose
    object files or archives are in the debug map (`N_OSO` entries) of
    `input_file`. The file is only read.
    """
    with open(input_file, "rb") as f:
        debug_file_paths = load_debug_file_paths(f) or []
    return sorted(
        {
            output_path
            for debug_file_path in debug_file_paths
            for output_path in get_target_output_paths(debug_file_path.path)
        }
    )


def write_dsymutil_allowlist(paths: list[str], output_file: str) -> None:
    """
    Writes the YAML file consumed by `dsymutil --allow`. JSON is a subset of
    YAML, so the file is emitted as JSON to get correct quoting of any path.
    """
    content = {"objects": [{"filename": path} for path in paths]}
    with open(output_file, "w") as f:
        json.dump(content, f, indent=2)
        f.write("\n")
