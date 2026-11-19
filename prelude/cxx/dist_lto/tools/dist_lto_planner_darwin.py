#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
A wrapper script around the linker invocation that completes distributed thin-lto thin-link.

This script:

(1) Parses the meta file, which specifies various file paths for each input object file.
(2) Writes "plan" json files for each input object file or archive describing to Buck starlark logic how to create opt + codegen actions for each
(3) Writes a "link plan" and a "final index". The link plan is used to identify which input object files are already native object files, and the final index constitutes a filelist used in the final native link. This filelist is a transformed version of the "index.full" file the linker produces.

Starlark code holds a representation of each input object file or archive in memory in an array. When code here needs to communicate characteristics about a particular element of this array, it encodes this using the index into this array. These indices are referred to as "starlark array index"
"""

import argparse
import dataclasses
import json
import os
import os.path
import subprocess
import sys
import traceback
from dataclasses import dataclass


@dataclass
class ObjectFile:
    """An object files passed directly to the distributed link. The object file may or may not be lazy."""

    input_object_file_path: str
    output_index_shard_file_path: str
    output_plan_file_path: str
    starlark_array_index: int

    def __init__(
        self,
        input_object_file_path: str,
        output_index_shard_file_path: str,
        output_plan_file_path: str,
        starlark_array_index: int,
    ):
        self.input_object_file_path = input_object_file_path
        self.output_index_shard_file_path = output_index_shard_file_path
        self.output_plan_file_path = output_plan_file_path
        self.starlark_array_index = int(starlark_array_index)


@dataclass
class ObjectFileOptimizationPlan:
    """A structure holding the information required to optimize and codegen a single bitcode object file passed
    directly to the link"""

    # A list of indices into the sorted_index_link_data starlark array
    # that identify other bitcode files this bitcode file imports from,
    # and thus should be optimized along side
    imports: list[int]
    is_bitcode: bool
    loaded_by_linker: bool
    final_link_line_position: int


BITCODE_SUFFIX = ".thinlto.bc"
IMPORTS_SUFFIX = ".imports"
OPT_OBJECTS_SUFFIX = ".opt.o"  # please note the files are not exist yet, this is to generate the index file use in final link


def _parse_meta_file_records(
    meta_file_path: str,
) -> tuple[dict[str, ObjectFile]]:
    object_file_records_map: dict[str, ObjectFile] = {}

    with open(meta_file_path) as meta:
        object_file_records_json = json.load(meta)
        for record in object_file_records_json:
            object_file_record = ObjectFile(**record)
            object_file_records_map[object_file_record.input_object_file_path] = (
                object_file_record
            )

    return object_file_records_map


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--meta")
    parser.add_argument("--index")
    parser.add_argument("--link-plan")
    parser.add_argument("index_args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv[1:])

    def read_imports(imports_path):
        with open(imports_path) as infile:
            return [line.strip() for line in infile.readlines()]

    def index_path(path):
        return os.path.join(args.index, path)

    object_file_records_map = _parse_meta_file_records(args.meta)

    subprocess.check_call(args.index_args[1:])

    loaded_input_files_positions = {}
    with open(index_path("index.full")) as indexfile:
        for position, line in enumerate(indexfile):
            if line.startswith(args.index):
                # Bitcode files that appear in the index are prefixed with the argument to --thinlto-prefix-replace
                # which in our case is the directory index is placed in. Remove this prefix to get the original path.
                input_bitcode_file_path = os.path.relpath(
                    line.strip(), start=args.index
                )
                loaded_input_files_positions[input_bitcode_file_path] = position
            else:
                loaded_input_files_positions[line.strip()] = position

    def _input_file_path_is_loaded_by_linker(path):
        return path in loaded_input_files_positions

    non_lto_objects = {}

    # Generate plans for object files
    for path, data in sorted(object_file_records_map.items(), key=lambda v: v[0]):
        # The linker will not write the sharded index to the location buck expects it to be by default, we need
        # to move it there.
        final_sharded_index_output_path = data.output_index_shard_file_path
        os.makedirs(os.path.dirname(final_sharded_index_output_path), exist_ok=True)

        temporary_sharded_index_location = index_path(path) + BITCODE_SUFFIX
        imports_file_path = index_path(path) + IMPORTS_SUFFIX

        final_link_line_position = -1
        if path in loaded_input_files_positions:
            final_link_line_position = loaded_input_files_positions[path]

        # import files are only written for bitcode files
        if os.path.exists(imports_file_path):
            assert os.path.exists(temporary_sharded_index_location), (
                "missing sharded index file for %s" % path
            )
            os.rename(temporary_sharded_index_location, final_sharded_index_output_path)

            imports = read_imports(imports_file_path)
            imports_list = []
            for import_path in imports:
                imported_object_file_record = object_file_records_map[import_path]
                imports_list.append(imported_object_file_record.starlark_array_index)

            plan = ObjectFileOptimizationPlan(
                imports=imports_list,
                is_bitcode=True,
                loaded_by_linker=_input_file_path_is_loaded_by_linker(path),
                final_link_line_position=final_link_line_position,
            )
        else:
            # The linker will not generate an index shard if the input is not bitcode.
            # Buck still expect the output, so write an empty file.
            non_lto_objects[data.starlark_array_index] = 1
            with open(final_sharded_index_output_path, "w"):
                pass

            plan = ObjectFileOptimizationPlan(
                imports=[],
                is_bitcode=False,
                loaded_by_linker=_input_file_path_is_loaded_by_linker(path),
                final_link_line_position=final_link_line_position,
            )

        with open(data.output_plan_file_path, "w") as planout:
            json.dump(dataclasses.asdict(plan), planout, sort_keys=True)

    # Dump the set of input object files that were already native object files instead of bitcode
    with open(args.link_plan, "w") as outfile:
        json.dump(
            {
                "non_lto_objects": non_lto_objects,
            },
            outfile,
            sort_keys=True,
        )


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except subprocess.CalledProcessError as e:
        traceback.print_exc()
        sys.exit(e.returncode)
