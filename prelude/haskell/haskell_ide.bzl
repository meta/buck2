# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "LinkableDepType", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")

def haskell_ide_impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo()]

haskell_ide = prelude_rule(
    name = "haskell_ide",
    impl = haskell_ide_impl,
    docs = """
        A `haskell_ide()` rule defines a project set for use with an
        LSP-based Haskell IDE (such as `ghcide`/HLS). When built, it
        compiles the dependencies of the listed sources and produces the
        GHC flags needed to load them as a single multi-component IDE
        session, which is faster than loading each project individually
        and avoids `ghcide`'s multi-component limitations.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "compiler_flags": attrs.list(attrs.string(), default = []),
            "deps": attrs.list(attrs.dep(), default = []),
            "deps_query": attrs.option(attrs.query(), default = None),
            "extra_script_templates": attrs.list(attrs.source(), default = []),
            "link_style": attrs.enum(LinkableDepType),
            "linker_flags": attrs.list(attrs.arg(), default = []),
            "platform": attrs.option(attrs.string(), default = None),
            "srcs": attrs.named_set(attrs.source(), sorted = True, default = []),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "include_projects": attrs.list(attrs.dep(), default = []),
            "_haskell_toolchain": toolchains_common.haskell(),
        }
    ),
)
