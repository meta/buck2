# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:preprocessor.bzl", "CPreprocessorInfo")
load(":providers.bzl", "LeanLibraryInfo", "LeanToolchainInfo")

def _lean_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        LeanToolchainInfo(
            lean = ctx.attrs.lean[RunInfo],
            lean_flags = ctx.attrs.lean_flags,
            linker_flags = ctx.attrs.linker_flags,
            c_compiler_flags = ctx.attrs.c_compiler_flags,
            c_export_flags = ctx.attrs.c_export_flags,
            c_preprocessor_infos = [dep[CPreprocessorInfo] for dep in ctx.attrs.c_headers],
            stdlib = ctx.attrs.stdlib,
            runtime = ctx.attrs.runtime,
        ),
    ]

lean_toolchain = rule(
    impl = _lean_toolchain_impl,
    attrs = {
        "c_compiler_flags": attrs.list(attrs.arg(), default = [], doc = "Flags for every compilation of generated C."),
        "c_export_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = "Flags that make the generated C export every symbol, for example `-DLEAN_EXPORTING`. `lean_library` then compiles the C of each module a second time with them, for shared libraries of Lean code on Windows, where an executable cannot export the whole core library. Leave it empty when `c_compiler_flags` exports the symbols already.",
        ),
        "c_headers": attrs.list(
            attrs.dep(providers = [CPreprocessorInfo]),
            default = [],
            doc = "C/C++ libraries that export `lean/lean.h` and the headers it includes.",
        ),
        "lean": attrs.exec_dep(providers = [RunInfo], doc = "The `lean` compiler."),
        "lean_flags": attrs.list(attrs.arg(), default = [], doc = "Flags for every `lean` invocation."),
        "linker_flags": attrs.list(
            attrs.arg(), default = [], doc = "Flags for every link of a `lean_binary` or `lean_test`, for example the stack size on Windows."
        ),
        "runtime": attrs.list(attrs.dep(), default = [], doc = "Native libraries that generated C links against."),
        "stdlib": attrs.list(
            attrs.dep(providers = [LeanLibraryInfo]),
            default = [],
            doc = "`lean_library` targets that every library depends on, for example the one with `Init`.",
        ),
    },
    is_toolchain_rule = True,
)
