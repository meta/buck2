# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
CythonToolchainInfo = provider(
    fields = {
        # The cython compiler binary (RunInfo provider).
        # Python version-based selection should be handled via select() + py_version_select()
        # on the compiler attribute in the toolchain BUCK definition, mirroring the
        # fbsource//third-party/pypi/cython:compiler alias pattern.
        "compiler": provider_field(RunInfo),
        # Default compiler flags. Currently unused by cython_library and
        # cython_static_extension (flags are built from rule attributes instead).
        "default_flags": provider_field(list[str]),
    }
)

def cython_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        CythonToolchainInfo(
            compiler = ctx.attrs.compiler[RunInfo],
            default_flags = ctx.attrs.default_flags,
        ),
    ]

cython_toolchain = prelude_rule(
    name = "cython_toolchain",
    impl = cython_toolchain_impl,
    docs = """
        A toolchain rule that provides the Cython compiler configuration.

        Python version-based compiler selection is handled via select() +
        py_version_select() on the "compiler" attribute in the toolchain
        BUCK definition, mirroring the fbsource//third-party/pypi/cython:compiler
        alias pattern.
    """,
    is_toolchain_rule = True,
    attrs = {
        "compiler": attrs.exec_dep(
            providers = [RunInfo],
            doc = """
            The Cython compiler binary target. Use select() + py_version_select()
            to vary the compiler by Python version.
        """,
        ),
        "default_flags": attrs.list(
            attrs.string(),
            default = [],
            doc = """
            Default flags to pass to the Cython compiler.
        """,
        ),
    },
)
