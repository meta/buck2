# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def cxx_toolchain_macro_impl(cxx_toolchain_rule = None, **kwargs):
    # `generate_linker_maps` set in order of priority:
    # - Explicit attribute on the cxx_toolchain() target
    # - Constraint on the target platform
    # - `cxx.linker_map_enabled` buckconfig
    if "generate_linker_maps" not in kwargs:
        linker_map_enabled = read_root_config("cxx", "linker_map_enabled", "").lower() == "true"
        kwargs["generate_linker_maps"] = select({
            "DEFAULT": linker_map_enabled,
            "config//linker/constraints:generate_linker_maps_disabled": False,
            "config//linker/constraints:generate_linker_maps_enabled": True,
        })

    bitcode = read_root_config("cxx", "bitcode")
    if bitcode != None:
        if bitcode.lower() == "false":
            kwargs["object_format"] = "native"
        elif bitcode.lower() == "true":
            kwargs["object_format"] = "bitcode"
        elif bitcode.lower() == "embed":
            kwargs["object_format"] = "embedded-bitcode"
        else:
            kwargs["object_format"] = "native"

    cxx_toolchain_rule(**kwargs)
