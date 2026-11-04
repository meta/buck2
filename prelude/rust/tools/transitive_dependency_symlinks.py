#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A tool to produce a directory structure compatible with rustc's `-Ldependency`
# flag for simple and dynamically named crates.
#
# Rustc needs all the .rlib files which are transitive dependencies of the crate
# being built. Importantly, the filename for each rlib must contain the correct
# crate name or else rustc won't find the file.
#
# Most crates have a crate name determined during analysis: value of the `crate`
# attribute, or derived using a simple heuristic from the rust_library target's
# `name` attribute. For these it's easy to give the rlib a correct filename up
# front and use buck's `ctx.actions.symlinked_dir` to collect them into a
# directory. These do not go through this tool.
#
# Crates that use `crate_dynamic` have a crate name computed at build time, for
# example by extracting the name from a .thrift file. Buck needs a filename at
# analysis time for all artifacts, so we name those rlib files using a
# provisional name and then this tool at build time will symlink them under the
# real crate name that rustc will recognize.
#
# Example:
#
#     transitive_dependency_symlinks.py \
#         --out-dir path/to/out \
#         --artifacts path/to/artifacts.json
#
# The input file artifacts.json is an array of pairs.
# The first element is the rlib paths, the second element is an optional path to a file containing
# the build-time crate name for the corresponding rlib.
#
#     [
#         [ "../../libprovisional.rlib", "path/to/cratename" ],
#         [ "../../libfoo.rlib", null ],
#         ...
#     ]
#
# The tool reads the crate name from the file at "path/to/cratename". Suppose it's
# "thriftgenerated". It symlinks the given artifact as "0/libthriftgenerated.rlib"
# within the specified output directory. In the event of collisions, there might
# be multiple dirs created.
#
# If the cratename is null, then the artifact is simply symlinked from the basename
# (e.g., "0/libprovisional.rlib").
#
# --out-dir-relative-to-cwd is an optional path to the output directory that is relative to
#                           the current working directory (cwd) of the tool that consumes this script's output.
#                           This path is the the output directory path used in -Ldependency.

import argparse
import json
import os
from pathlib import Path
from typing import IO, NamedTuple, Optional


class Args(NamedTuple):
    name: str
    out_dir: Path
    out_dir_relative_to_cwd: Optional[Path]
    artifacts: IO[str]


def symlink_relative(link: Path, target: Path) -> None:
    """Create a symlink from link_name to target.

    The link is created by constructing a path to the target that is relative to link_name's parent.
    """
    target_relative = os.path.relpath(target, start=os.path.dirname(link))
    os.symlink(target_relative, link)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--out-dir",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--out-dir-relative-to-cwd",
        type=Path,
        required=False,
    )
    parser.add_argument(
        "--artifacts",
        type=argparse.FileType(),
        required=True,
    )
    parser.add_argument(
        "--name",
        type=str,
        required=True,
    )
    args = Args(**vars(parser.parse_args()))

    args.out_dir.mkdir(exist_ok=True)

    # Add as many -Ldependency dirs as we need to avoid name conflicts
    deps_dirs = [{}]

    for artifact, crate_name in json.load(args.artifacts):
        is_dynamic = crate_name is not None
        if is_dynamic:
            crate_name = Path(crate_name).read_text().strip()
            original_filename = os.path.basename(artifact)
            new_filename = "lib{}-{}".format(
                crate_name,
                original_filename.rsplit("-", 1)[1],
            )
            filename = new_filename
        else:
            filename = os.path.basename(artifact)

        if filename in deps_dirs[-1]:
            deps_dirs.append({})
        deps_dirs[-1][filename] = artifact

    flags = ""

    for idx, srcs in enumerate(deps_dirs):
        if srcs:
            directory = args.out_dir.joinpath(str(idx))
            directory.mkdir()

            dependency_path = (
                args.out_dir_relative_to_cwd.joinpath(str(idx))
                # The -Ldependency output path gets a relative path if there is one,
                # because the tools consuming this expect a path relative to cwd.
                if args.out_dir_relative_to_cwd
                else directory
            )

            flags += "-Ldependency={}\n".format(dependency_path)
            for filename, artifact in srcs.items():
                # Create the symlinks in the directory (i.e., not the relative path)
                # because this script is NOT executing relative to the cwd.
                symlink_relative(directory.joinpath(filename), artifact)

    args.out_dir.joinpath("dirs").write_text(flags)


if __name__ == "__main__":
    main()
