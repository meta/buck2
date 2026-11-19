# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _srcs_arg():
    return {
        "srcs": attrs.named_set(
            attrs.source(),
            sorted = True,
            default = [],
            doc = """
    A list of Haskell sources to be built by this rule. The dictionary option is deprecated.
""",
        ),
    }

def _deps_arg():
    return {
        "deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    Either `haskell_library()` or `prebuilt_haskell_library()` rules
     from which this rules sources import modules or native linkable rules exporting symbols
     this rules sources call into.
""",
        ),
    }

def _compiler_flags_arg():
    return {
        "compiler_flags": attrs.list(
            attrs.string(),
            default = [],
            doc = """
    Flags to pass to the Haskell compiler when compiling this rule's sources.
""",
        ),
    }

def _exported_linker_flags_arg():
    return {
        "exported_linker_flags": attrs.list(
            attrs.string(),
            default = [],
            doc = """
    Linker flags used by dependent rules when linking with this library.
""",
        ),
    }

haskell_common = struct(
    srcs_arg = _srcs_arg,
    deps_arg = _deps_arg,
    compiler_flags_arg = _compiler_flags_arg,
    exported_linker_flags_arg = _exported_linker_flags_arg,
)
