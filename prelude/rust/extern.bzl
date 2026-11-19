# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    ":crate_name.bzl",
    "CrateName",
    "crate_name_as_cmd_arg",
)

# Create `--extern` flag. For crates with a name computed during analysis:
#
#     --extern=NAME=path/to/libNAME.rlib
#
# For crates with a name computed during build:
#
#     --extern=$(cat path/to/REALNAME)=path/to/libPROVISIONAL.rlib
#
def extern_arg(flags: list[str], crate: CrateName, lib: Artifact) -> cmd_args:
    if flags == []:
        flags = ""
    else:
        flags = ",".join(flags) + ":"

    if type(crate.simple) == "string" and crate.dynamic == None:
        return cmd_args(
            lib,
            format = "--extern={}{}={{}}".format(flags, crate.simple),
        )

    return cmd_args(
        "--extern=",
        flags,
        crate_name_as_cmd_arg(crate),
        "=",
        lib,
        delimiter = "",
    )

# Create `--crate-map` flag. For crates with a name computed during analysis:
#
#     --crate-map=NAME=//path/to:target
#
# For crates with a name computed during build:
#
#     --crate-map=$(cat path/to/REALNAME)=//path/to:target
#
def crate_map_arg(crate: CrateName, label: Label) -> cmd_args | str:
    if type(crate.simple) == "string" and crate.dynamic == None:
        return "--crate-map={}={}".format(crate.simple, str(label.raw_target()))

    return cmd_args(
        "--crate-map=",
        crate_name_as_cmd_arg(crate),
        "=",
        str(label.raw_target()),
        delimiter = "",
    )
