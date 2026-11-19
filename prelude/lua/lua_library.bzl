# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load(":attrs.bzl", "lua_common")

def lua_library_impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo()]

lua_library = prelude_rule(
    name = "lua_library",
    impl = lua_library_impl,
    docs = """
        A `lua_library()` rule is used to group together Lua sources
        to be packaged into a top-level `lua_binary()` rule.
    """,
    examples = """
        ```
        # A rule that includes a single .py file.
        lua_library(
          name = 'fileutil',
          srcs = ['fileutil.lua'],
        )

        # A rule that uses glob() to include all sources in the directory which the
        # rule is defined.  It also lists a resource file that gets packaged with
        # the sources in this rule.
        lua_library(
          name = 'testutil',
          srcs = glob(['testutil/**/*.lua'],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        lua_common.srcs_arg()
        | lua_common.base_module_arg()
        | {
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Other `lua_library()` rules which list `srcs` from
                 which this rule imports modules.
            """,
            ),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
