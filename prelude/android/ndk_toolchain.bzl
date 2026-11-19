# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxPlatformInfo", "CxxToolchainInfo")
load("@prelude//decls:common.bzl", "buck")

NdkCxxRuntime = ["system", "gabixx", "stlport", "gnustl", "libcxx"]

ndk_toolchain = prelude_rule(
    name = "ndk_toolchain",
    # Has no implementation: targets of this type can be parsed and queried, but not built.
    docs = "",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "cxx_runtime": attrs.option(attrs.enum(NdkCxxRuntime), default = None),
            "cxx_toolchain": attrs.toolchain_dep(providers = [CxxToolchainInfo, CxxPlatformInfo]),
            "objdump": attrs.source(),
            "shared_runtime_path": attrs.option(attrs.source(), default = None),
            "strip_apk_libs_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
