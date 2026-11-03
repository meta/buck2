#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
A simple wrapper around a distributed thinlto index command to fit into buck2's
distributed thinlto build.

This reads in a couple of things:
    1. The "meta" JSON file. This is a list of all the linkable information.
       It contains the object file, index output, plan output.
       As well as associated pre/post flags for linkables.
    2. The index and link plan output paths
    3. The commands for the actual index command.

It will invoke the index command and then copy the index outputs to the
requested locations and write a plan for each of those objects. This "plan" is
a simple json file with the most important thing being a list of the indices
of the imports needed for that file.

It will then additionally write a link plan, which is just a translation of
the thinlto index (which lists the objects actually needed for the final link).


Both opt and link plans use indices to refer to other files because it allows the bzl
code to easily map back to other objects held in buck memory.
"""

import argparse
import json
import os
import os.path
import resource
import subprocess
import sys
import traceback
from typing import List, TextIO


def _flatten_deep(items):
    """Flatten recursive list of lists to a single list.

    Example:
    >>> _flatten_deep([1, [2, 3], 4, [5, [6]]]) == [1, 2, 3, 4, 5, 6]
    """

    def flatten_deep_helper(items, result):
        for item in items:
            if isinstance(item, (list, tuple)):
                flatten_deep_helper(item, result)
            else:
                result.append(item)

    result = []
    flatten_deep_helper(items, result)

    return result


def _get_argsfile(args) -> str:
    # go through the flags passed to linker and find the index argsfile
    argsfiles = list(
        filter(lambda arg: arg.endswith("thinlto_index_argsfile"), args.index_args)
    )
    assert len(argsfiles) == 1, (
        f"expect only 1 argsfile but seeing multiple ones: {argsfiles}"
    )
    argsfile = argsfiles[0]
    if argsfile.startswith("@"):
        argsfile = argsfile[1:]
    return argsfile


def _extract_lib_search_path(argsfile_path: str) -> List[str]:
    lib_search_path = []
    with open(argsfile_path) as argsfile:
        for line in argsfile:
            if line.startswith("-L"):
                lib_search_path.append(line.strip())
    return lib_search_path


def _write_args(argsfile: TextIO, args: List[str]) -> None:
    for arg in args:
        argsfile.write(arg.strip())
        argsfile.write("\n")


def _enable_core_dumps() -> None:
    # The index step runs the system linker, which -- unlike Meta binaries --
    # installs no userspace crash handler, so a crash only leaves a core dump if
    # the kernel writes one, and the kernel writes nothing while the RLIMIT_CORE
    # soft limit is 0. buck2 sets no per-action rlimits, so raise the soft limit
    # to the inherited hard limit here (best effort: a no-op when the hard limit
    # is itself 0, which needs a host/container-level change instead).
    try:
        _, hard = resource.getrlimit(resource.RLIMIT_CORE)
        resource.setrlimit(resource.RLIMIT_CORE, (hard, hard))
    except (OSError, ValueError) as e:
        print(f"warning: failed to enable core dumps: {e}", file=sys.stderr)


def _reject_bitcode_without_summary(path, bc_file):
    # LLD writes a zero-byte index for a bitcode module it linked but could not
    # thin-link: bitcode without a module summary (`-flto` rather than
    # `-flto=thin`). The module is neither in the linked-objects index nor
    # compilable by a distributed backend; a skip-flag index is never empty.
    if os.path.getsize(bc_file) == 0:
        raise AssertionError(
            f"LLD linked bitcode without a ThinLTO summary: {path} "
            "(compiled with -flto rather than -flto=thin?)"
        )


def _validate_linked_bitcode_outputs(
    linked_paths, mapped_paths, index_path, bitcode_suffix, imports_suffix
):
    for path in sorted(linked_paths & mapped_paths.keys()):
        imports_path = index_path(path) + imports_suffix
        if not os.path.exists(imports_path):
            raise AssertionError(
                f"LLD selected ThinLTO module without imports file: {path}"
            )
        bc_file = index_path(path) + bitcode_suffix
        if not os.path.exists(bc_file):
            raise AssertionError(
                f"LLD selected ThinLTO module without bitcode index: {path}"
            )


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--meta")
    parser.add_argument("--index")
    parser.add_argument("--link-plan")
    parser.add_argument("--final-link-index")
    parser.add_argument("index_args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv[1:])

    _enable_core_dumps()
    subprocess.check_call(args.index_args[1:])

    bitcode_suffix = ".thinlto.bc"
    imports_suffix = ".imports"
    opt_objects_suffix = ".opt.o"  # please note the files are not exist yet, this is to generate the index file use in final link

    # `linkables_index` contains the linkables that are NOT LLVM IR bitcode files (e.g., machine code archives, shared libraries).
    # `linkables_index`'s key is the path to the linkable and the value is a dictionary containing the cmd_args and the meta_index.
    # `linkables_index` is used to re-attach associated flags (pre/post flags and flags in the cmd_args form like `-Wl,--whole-archive`/`-Wl,--no-whole-archive`)
    # for the final_link_index @argsfile used in the the Phase 4 final link.
    # The associated flags are otherwise dropped because `ld.lld` produces an `index.full` file in the Phase 2 Thin Link step
    # only contains the path to the linkable and no flags.
    linkables_index = {}

    with open(args.meta) as f:
        meta = json.load(f)

    for meta_entry_index1, meta_entry1 in enumerate(meta):
        for linkable in meta_entry1["linkables"]:
            if linkable["type"] == "cmd_args":
                obj = linkable["object"]

                if obj["type"] == "archive":
                    linkable_path = obj["archive"]
                    linkables_index[linkable_path] = {
                        "cmd_args": linkable["cmd_args"],
                        "meta_index": meta_entry_index1,
                    }
                elif obj["type"] == "shared_lib":
                    linkable_path = obj["name"]
                    linkables_index[linkable_path] = {
                        "cmd_args": linkable["cmd_args"],
                        "meta_index": meta_entry_index1,
                    }
                else:
                    raise Exception(f"unknown linkable type: {obj}")

    def read_imports(path, imports_path):
        with open(imports_path) as infile:
            return [line.strip() for line in infile.readlines()]

    def index_path(path):
        return os.path.join(args.index, path)

    # The meta file comes directly from dist_lto.bzl and consists of a list of JSON objects.
    #
    # Example:
    # ```
    # [
    #    {
    #       "linkables": [
    #          {
    #             "type": "bitcode",
    #             "path": "buck-out/v2/art/spongebob/Gary.cpp.o",
    #             "output": "buck-out/v2/art/spongebob/Gary.cpp.o.thinlto.bc",
    #             "plan_output": "buck-out/v2/art/spongebob/Gary.cpp.o.opt.plan",
    #             "idx": 0
    #          },
    #          ...
    #        ],
    #        "pre_flags": [],
    #        "post_flags": [],
    #    },
    #    {
    #       "linkables": [
    #         {
    #            "type": "cmd_args",
    #            "cmd_args": [ "build/glog/lib/libglog.a" ],
    #            "linkable_type": "archive_linkable",
    #            "object": {
    #              "type": "archive",
    #              "archive": "build/glog/lib/libglog.a",
    #            }
    #         }
    #       ]
    #     },
    #    ...
    # ]
    # ```
    #
    # Inside objects with type "bitcode" and "archive", there are several attributes:
    #  - path: The path to the source bitcode file from Phase 1 Compile.
    #        This is used as an index into a dictionary (`mapping`) that records much of the metadata coming
    #        from these lines.
    #  - output: The path to output location of Phase 2 Thin Link's analysis result for this individual module.
    #        This script is expected to place a ThinLTO index file at this location (suffixed `.thinlto.bc`).
    #  - plan_output: The path to an output plan. This script is expected to place a link
    #        plan here (a JSON document indicating which other object files this)
    #        object file depends on, among other things.
    #  - idx: The link data's index in the Starlark array.
    #  - archive_name: If this object file came from an archive, the name of the archive.
    #        Otherwise, this line is empty.
    #  - archive_plan: If this object file came from an archive, the path to an output plan.
    #        This script is expected to produce an archive link plan here (a JSON)
    #        document similar to the object link plan, except containing link
    #        information for every file in the archive from which this object
    #        came. Otherwise, this line is empty.
    #  - archive_index_dir: If this object file came from an archive, the indexes directory of that
    #        archive. This script is expected to place all ThinLTO indexes derived
    #        from object files originating from this archive in that directory.
    #        Otherwise, this line is empty.
    #  - archive_opt_objects_dir: If this object file came from an archive, the directory where
    #        Phase 3 stages every member of that archive, each at its path relative
    #        to the archive's base dir (the dir holding `archive_plan`). It is the
    #        only dir of the archive that Phase 4 Final Link materializes, so the
    #        final-link index must refer to members through it.
    #  - link_whole: Whether every member of this archive must be linked.
    #
    # There are two indices that are derived from this meta file: the object
    # index (mapping["index"]) and the archive index (mapping["archive_index"]).
    # These indices are indices into Starlark arrays for all objects and archive
    # linkables, respectively. This script does not inspect them.
    mapping = {}
    archives = {}
    for meta_entry_index2, meta_entry2 in enumerate(meta):
        for linkable in meta_entry2["linkables"]:
            if linkable["type"] == "bitcode":
                path = linkable["path"]
                output = linkable["output"]
                plan_output = linkable["plan_output"]
                idx = linkable["idx"]
                mapping[path] = {
                    "output": output,
                    "plan_output": plan_output,
                    "index": idx,
                    "meta_index": meta_entry_index2,
                    "archive_index": None,
                    "archive_name": "",
                    "opt_object_path": "",
                }
            elif linkable["type"] == "archive":
                archive_idx = linkable["archive_idx"]
                archive_name = linkable["archive_name"]
                archive_plan = linkable["archive_plan"]
                archive_index_dir = linkable["archive_index_dir"]
                archive_opt_objects_dir = linkable["archive_opt_objects_dir"]
                archive_base_dir = os.path.dirname(archive_plan)

                obj_mapping_entry = {
                    "output": "",
                    "plan_output": "",
                    "index": archive_idx,
                    "meta_index": meta_entry_index2,
                    "archive_index": archive_idx,
                    "archive_name": archive_name,
                }
                for obj in linkable["objects"] or ():
                    path = obj["path"]
                    mapping[path] = dict(
                        obj_mapping_entry,
                        # Where Phase 3 stages this member, and thus the path Phase 4
                        # Final Link must use for it.
                        opt_object_path=os.path.join(
                            archive_opt_objects_dir,
                            os.path.relpath(path, archive_base_dir),
                        ),
                    )

                archives[archive_idx] = {
                    "name": archive_name,
                    "objects": [],
                    "plan": archive_plan,
                    "index_dir": archive_index_dir,
                    "link_whole": linkable["link_whole"],
                }

    mapping_by_normalized_path = {}
    original_path_by_normalized_path = {}
    for path, data in mapping.items():
        normalized_path = os.path.normpath(path)
        if normalized_path in mapping_by_normalized_path:
            raise RuntimeError(
                "ThinLTO inputs normalize to the same path: {!r} and {!r}".format(
                    original_path_by_normalized_path[normalized_path], path
                )
            )
        mapping_by_normalized_path[normalized_path] = data
        original_path_by_normalized_path[normalized_path] = path

    # LLD emits index sidecars for unselected lazy bitcode members. Only paths
    # in the linked-objects index require a distributed backend.
    index = {}
    linked_bitcode_paths = set()
    with open(index_path("index")) as indexfile:
        for line in indexfile:
            path = os.path.normpath(os.path.relpath(line.strip(), start=args.index))
            linked_bitcode_paths.add(path)
            if path in mapping_by_normalized_path:
                index[mapping_by_normalized_path[path]["index"]] = 1

    _validate_linked_bitcode_outputs(
        linked_bitcode_paths,
        mapping_by_normalized_path,
        index_path,
        bitcode_suffix,
        imports_suffix,
    )

    non_lto_objects = {}
    for path, data in sorted(mapping.items(), key=lambda v: v[0]):
        output_loc = data["output"]
        if os.path.exists(output_loc):
            continue

        if data["archive_index"] is not None:
            archives[data["archive_index"]]["objects"].append(path)
            continue

        bc_file = index_path(path) + bitcode_suffix
        imports_path = index_path(path) + imports_suffix
        os.makedirs(os.path.dirname(output_loc), exist_ok=True)

        if os.path.exists(imports_path):
            assert os.path.exists(bc_file), "missing bc file for %s" % path
            _reject_bitcode_without_summary(path, bc_file)
            os.rename(bc_file, output_loc)
            imports = read_imports(path, imports_path)
            imports_list = []
            archives_list = []
            for import_path in imports:
                if import_path not in mapping:
                    continue
                entry = mapping[import_path]
                if entry["archive_index"] is not None:
                    archives_list.append(int(entry["archive_index"]))
                else:
                    imports_list.append(entry["index"])
            plan = {
                "imports": imports_list,
                "archive_imports": archives_list,
                "index": data["index"],
                "bitcode_file": bc_file,
                "path": path,
                "is_bc": True,
                "skip_backend": os.path.normpath(path) not in linked_bitcode_paths,
            }
        else:
            non_lto_objects[data["index"]] = 1
            with open(output_loc, "w"):
                pass
            plan = {
                "is_bc": False,
                "skip_backend": False,
            }

        with open(data["plan_output"], "w") as planout:
            json.dump(plan, planout, sort_keys=True)

    for archive in archives.values():
        # For archives, we must produce a plan that provides Starlark enough
        # information about how to launch a dynamic opt for each object file
        # in the archive.
        archive_plan = {}

        # This is convenient to store, since it's difficult for Starlark to
        # calculate it.
        archive_plan["base_dir"] = os.path.dirname(archive["plan"])
        object_plans = []
        for obj in archive["objects"]:
            imports_path = index_path(obj) + imports_suffix
            output_path = archive["index_dir"]
            os.makedirs(output_path, exist_ok=True)
            if os.path.exists(imports_path):
                bc_file = index_path(obj) + bitcode_suffix
                _reject_bitcode_without_summary(obj, bc_file)
                skip_backend = os.path.normpath(obj) not in linked_bitcode_paths
                if skip_backend and archive["link_whole"]:
                    raise AssertionError(
                        f"LLD skipped whole-archive ThinLTO member {obj}"
                    )
                if skip_backend:
                    object_plans.append(
                        {
                            "is_bc": True,
                            "path": obj,
                            "skip_backend": True,
                        }
                    )
                    continue
                os.rename(bc_file, os.path.join(output_path, os.path.basename(bc_file)))
                imports = read_imports(obj, imports_path)

                imports_list = []
                archives_list = []
                for path in imports:
                    if path not in mapping:
                        continue
                    entry = mapping[path]
                    if entry["archive_index"] is not None:
                        archives_list.append(int(entry["archive_index"]))
                    else:
                        imports_list.append(entry["index"])
                object_plans.append(
                    {
                        "is_bc": True,
                        "path": obj,
                        "imports": imports_list,
                        "archive_imports": archives_list,
                        "bitcode_file": os.path.join(
                            output_path, os.path.basename(bc_file)
                        ),
                        "skip_backend": False,
                    }
                )
            else:
                object_plans.append(
                    {
                        "is_bc": False,
                        "path": obj,
                        "skip_backend": False,
                    }
                )

        archive_plan["objects"] = object_plans
        with open(archive["plan"], "w") as planout:
            json.dump(archive_plan, planout, sort_keys=True)

    # We read the `index` and `index.full` files produced by linker in index stage
    # and translate them to 2 outputs:
    # 1. A link plan build final_link args. (This one may be able to be removed if we refactor the workflow)
    # 2. A files list (*.final_link_index) used for final link stage which includes all the
    #    files needed. it's based on index.full with some modification, like path updates
    #    and redundant(added by toolchain) dependencies removing.
    # The `index` file is a list of the LLVM IR bitcode files that were analyzed in the Phase 2 Thin Link Step.
    # The `index.full` file is of ALL linkables used in the linking process, ordered by their symbol resolution.
    # For a full discussion on `index.full` see https://reviews.llvm.org/D130229
    # The file paths in `index` and the LLVM IR Bitcode files in `index.full` are listed as a child of the `args.index` directory.
    # HOWEVER, the LLVM IR Bitcode files do NOT exist there and instead exist in the location with the `args.index` directory stripped off (i.e., with `os.path.relpath(start=args.index)`).
    # The files suffixed with `.imports` and `.thinlto.bc` DO exist in the directory under `args.index`.
    # TODO(T130322878): since we call linker wrapper twice (in index and in final_link), to avoid these libs get
    # added twice we remove them from the index file for now.
    KNOWN_REMOVABLE_DEPS_SUFFIX = [
        "glibc/lib/crt1.o",
        "glibc/lib/crti.o",
        "glibc/lib/Scrt1.o",
        "crtbegin.o",
        "crtbeginS.o",
        ".build_info.o",
        "crtend.o",
        "crtendS.o",
        "glibc/lib/crtn.o",
    ]
    GENERATED_BUILD_INFO_OBJECT_BASENAME_PREFIX = "__buck2_generated_build_info.c"

    with open(args.link_plan, "w") as outfile:
        json.dump(
            {
                "non_lto_objects": non_lto_objects,
                "index": index,
            },
            outfile,
            indent=2,
            sort_keys=True,
        )

    # Append all search path flags (e.g -Lfbcode/third-party-buck/platform010/build/glibc/lib) from argsfile to final_index
    # this workaround is to make dist_lto compatible with link_group. see T136415235 for more info
    argsfile = _get_argsfile(args)
    lib_search_path = _extract_lib_search_path(argsfile)

    # build index file for final link use
    with (
        open(index_path("index.full")) as full_index_input,
        open(args.final_link_index, "w") as final_link_index_output,
    ):
        final_link_index_output.write("\n".join(lib_search_path) + "\n")

        # Keep track of the flags that were written because the ones that weren't are written at the end.
        # The index into `flags_written_tracker` corresponds to the `index` into `meta`.
        flags_written_tracker = bytearray(len(meta))

        # Write out first flags without an associated linkable in the meta.
        # Stop once we see the first linkable because
        # flags with an associated linkables are written in the order they are seen in `full_index_input`.
        for meta_entry_index3, meta_entry3 in enumerate(meta):
            if meta_entry3["linkables"]:
                # Break once we see the first linkable entry.
                break
            else:
                pre_flags = _flatten_deep(meta_entry3["pre_flags"])
                _write_args(final_link_index_output, pre_flags)

                post_flags = _flatten_deep(meta_entry3["post_flags"])
                _write_args(final_link_index_output, post_flags)
                flags_written_tracker[meta_entry_index3] = 1

        # Keep track of the previous meta_index so when the meta_index changes between loop iterations
        # the post flags are written for the previous meta_index and pre_flags are written for the new meta_index.
        prev_meta_index = None

        for line in full_index_input:
            line = line.strip()
            if any(
                filter(line.endswith, KNOWN_REMOVABLE_DEPS_SUFFIX)
            ) or os.path.basename(line).startswith(
                GENERATED_BUILD_INFO_OBJECT_BASENAME_PREFIX
            ):
                continue
            # LLVM IR files that are indexed in Phase 2 Thin Link are reported by `ld.lld` to exist under the directory `args.index`,
            # but they actually exist at the path with the prefix `args.index` removed.
            # Remove the prefix `args.index` with `os.path.relpath`.
            path = os.path.normpath(os.path.relpath(line, start=args.index))
            mapping_entry = mapping_by_normalized_path.get(path)

            if mapping_entry is not None:
                meta_index = mapping_entry["meta_index"]
            elif line in mapping:
                # This case happens when a library is marked with `enable_distributed_thinlto = True`, but
                # has `compiler_flags = ["-fno-lto"]`.
                # In this case the .o file is a machine code file and thus it is NOT indexed by Phase 2 Thin Link,
                # and `line` is NOT prefixed with `args.index`.
                mapping_entry = mapping[line]
                meta_index = mapping_entry["meta_index"]
            elif line in linkables_index:
                mapping_entry = None
                meta_index = linkables_index[line]["meta_index"]
            else:
                mapping_entry = None
                meta_index = None

            if (
                mapping_entry is not None
                and path not in linked_bitcode_paths
                and os.path.exists(index_path(path) + imports_suffix)
            ):
                raise AssertionError(
                    f"LLD listed skipped ThinLTO member in index.full: {path}"
                )

            has_meta_entry = meta_index is not None

            if has_meta_entry:
                # For simplicity, mark now that we have written the flags for this entry.
                flags_written_tracker[meta_index] = 1

            write_post_flags = (
                prev_meta_index is not None and prev_meta_index != meta_index
            )
            if write_post_flags:
                if post_flags := meta[prev_meta_index]["post_flags"]:
                    _write_args(final_link_index_output, _flatten_deep(post_flags))

            write_pre_flags = has_meta_entry and (
                prev_meta_index is None or prev_meta_index != meta_index
            )
            if write_pre_flags:
                # Before writing the pre_flags for THIS entry,
                # write the preceding pre_flags/post_flags without linkables.
                # Since this iterates backwards, we'll store the flags in a stack,
                # and do a 2nd pass that writes them in the correct order into `final_index_args`.
                preceding_flags_stack = []
                for preceding_meta_index in range(meta_index - 1, -1, -1):
                    if flags_written_tracker[preceding_meta_index]:
                        break

                    preceding_meta_entry = meta[preceding_meta_index]
                    if preceding_meta_entry["linkables"]:
                        # Only write up-to the next linkable
                        break

                    if preceding_post_flags := preceding_meta_entry["post_flags"]:
                        preceding_flags_stack.append(preceding_post_flags)

                    if preceding_pre_flags := preceding_meta_entry["pre_flags"]:
                        preceding_flags_stack.append(preceding_pre_flags)

                    flags_written_tracker[preceding_meta_index] = 1

                for preceding_flags in reversed(preceding_flags_stack):
                    _write_args(final_link_index_output, _flatten_deep(preceding_flags))

                if pre_flags := meta[meta_index]["pre_flags"]:
                    _write_args(final_link_index_output, _flatten_deep(pre_flags))

            prev_meta_index = meta_index

            if path in linked_bitcode_paths and mapping_entry is None:
                # Bitcode the linker indexed that no linkable declared (an object
                # named in raw linker flags): passed through as the linker wrote it.
                final_link_index_output.write(line + "\n")
            elif path in linked_bitcode_paths and mapping_entry["output"]:
                # This case is for a known and true LLVM IR Bitcode file that was
                # NOT extracted from an archive.

                # Get the output path to the actual file that is linked in Phase 4 Final Link,
                # which is resulting file from Phase 3 ThinLTO Backends.
                # These files are suffixed with ".opt.o" and are siblings to the ".thinlto.bc" files,
                # so the path is obtained by replacing the suffix.
                output = mapping_entry["output"].replace(
                    bitcode_suffix, opt_objects_suffix
                )
                final_link_index_output.write(output + "\n")
            elif (
                mapping_entry is not None and mapping_entry["archive_index"] is not None
            ):
                # A member extracted from an LTO-able archive. Phase 3 stages every
                # member -- bitcode and machine code alike -- in the archive's
                # `opt_objects` dir, so both kinds are redirected there. Machine-code
                # members occur for e.g. Rust `compiler_builtins`, which rustc emits as
                # native objects even under `-Clinker-plugin-lto` (the crate is
                # `#![no_builtins]`), so `ld.lld` produces no ThinLTO index for them.
                final_link_index_output.write(mapping_entry["opt_object_path"] + "\n")
            elif line in linkables_index:
                # If this is a known linkable, then use the original cmd_args from the linkable entry,
                # which could contain for example, "-Wl,--whole-archive" and "-Wl,--no-whole-archive" flags.
                linkable_entry = linkables_index[line]
                cmd_args = linkable_entry["cmd_args"]
                if isinstance(cmd_args, list):
                    _write_args(final_link_index_output, _flatten_deep(cmd_args))
                else:
                    final_link_index_output.write(cmd_args + "\n")
            else:
                # handle:
                # - objects from genrule archives (not tracked by dist_lto)
                # - pre-built archives
                # - input files that did not come from linker input, e.g. linkerscirpts
                final_link_index_output.write(line + "\n")

        # Write post flags for the last iteration of the loop
        if prev_meta_index is not None:
            if post_flags := meta[prev_meta_index]["post_flags"]:
                _write_args(final_link_index_output, _flatten_deep(post_flags))

        # Write out the remaining unwritten flags
        for flags_written_tracker_index, flags_written_tracker_entry in enumerate(
            flags_written_tracker
        ):
            if not flags_written_tracker_entry:
                meta_entry5 = meta[flags_written_tracker_index]
                pre_flags = _flatten_deep(meta_entry5["pre_flags"])
                _write_args(final_link_index_output, pre_flags)
                post_flags = _flatten_deep(meta_entry5["post_flags"])
                _write_args(final_link_index_output, post_flags)


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except subprocess.CalledProcessError as e:
        traceback.print_exc()
        sys.exit(e.returncode)
