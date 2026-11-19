# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")

def lua_binary_impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo()]

LuaPlatformPackageStyle = ["standalone", "inplace"]

lua_binary = prelude_rule(
    name = "lua_binary",
    impl = lua_binary_impl,
    docs = """
        A `lua_binary()` rule is used to build an executable that uses Lua
        sources from all transitive `lua_library()` dependencies.
    """,
    examples = """
        ```
        lua_binary(
          name = 'tailer',
          main_module = 'tailer',
          deps = [
            ':tailerutils',
          ],
        )

        lua_library(
          name = 'tailerutils',
          srcs = glob(['*.lua']),
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "main_module": attrs.string(
                default = "",
                doc = """
                The module which serves as the entry point for this rule.
            """,
            ),
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                `lua_library()` rules to this binary will access.
            """,
            ),
            "native_starter_library": attrs.option(attrs.dep(), default = None),
            "package_style": attrs.option(attrs.enum(LuaPlatformPackageStyle), default = None),
            "platform": attrs.option(attrs.string(), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
