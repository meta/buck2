# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "CxxSourceType", "buck")
load(":attr_selection.bzl", "cxx_by_language_ext")

CxxFlags = record(
    compiler_flags = field(list[typing.Any], []),
    lang_compiler_flags = field(dict[typing.Any, typing.Any], {}),
    lang_preprocessor_flags = field(dict[typing.Any, typing.Any], {}),
    linker_flags = field(list[typing.Any], []),
    preprocessor_flags = field(list[typing.Any], []),
)

def _compiler_flags(flags: CxxFlags):
    return cmd_args(flags.compiler_flags)

def _linker_flags(flags: CxxFlags):
    return cmd_args(flags.linker_flags)

def _preprocessor_flags(flags: CxxFlags):
    return cmd_args(flags.preprocessor_flags)

def _flags_for_ext(ext: str):
    def project(flags: CxxFlags):
        return cmd_args(
            flags.preprocessor_flags,
            cxx_by_language_ext(flags.lang_preprocessor_flags, ext),
            cxx_by_language_ext(flags.lang_compiler_flags, ext),
            flags.compiler_flags,
        )

    return project

CxxFlagsTSet = transitive_set(
    args_projections = {
        "asm": _flags_for_ext(".asm"),
        "asmpp": _flags_for_ext(".S"),
        "c": _flags_for_ext(".c"),
        "compiler_flags": _compiler_flags,
        "cuda": _flags_for_ext(".cu"),
        "cxx": _flags_for_ext(".cpp"),
        "hip": _flags_for_ext(".hip"),
        "linker_flags": _linker_flags,
        "objc": _flags_for_ext(".m"),
        "objcxx": _flags_for_ext(".mm"),
        "preprocessor_flags": _preprocessor_flags,
    },
)

CxxFlagsInfo = provider(
    fields = {
        "flags": provider_field(typing.Any),  # CxxFlagsTSet
    },
)

def cxx_flags_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        CxxFlagsInfo(
            flags = ctx.actions.tset(
                CxxFlagsTSet,
                value = CxxFlags(
                    compiler_flags = ctx.attrs.compiler_flags,
                    lang_compiler_flags = ctx.attrs.lang_compiler_flags,
                    lang_preprocessor_flags = ctx.attrs.lang_preprocessor_flags,
                    linker_flags = ctx.attrs.linker_flags,
                    preprocessor_flags = ctx.attrs.preprocessor_flags,
                ),
                children = [dep[CxxFlagsInfo].flags for dep in ctx.attrs.deps],
            ),
        ),
    ]

def cxx_attr_flags(ctx: AnalysisContext) -> list[Dependency]:
    return getattr(ctx.attrs, "flags", [])

def cxx_flags_tset(actions: AnalysisActions, flag_deps: list[Dependency]):
    if not flag_deps:
        return None
    return actions.tset(
        CxxFlagsTSet,
        children = [dep[CxxFlagsInfo].flags for dep in flag_deps],
    )

def cxx_flags_projection_name(ext: str) -> str:
    if ext == ".c":
        return "c"
    elif ext in (".cpp", ".cc", ".cl", ".cxx", ".c++", ".bc"):
        return "cxx"
    elif ext == ".m":
        return "objc"
    elif ext == ".mm":
        return "objcxx"
    elif ext in (".s", ".sx", ".S"):
        return "asmpp"
    elif ext == ".cu":
        return "cuda"
    elif ext == ".hip":
        return "hip"
    elif ext in (".asm", ".asmpp"):
        return "asm"
    fail("Unexpected file extension: " + ext)

def cxx_flags_linker_flags(actions: AnalysisActions, flag_deps: list[Dependency]) -> list[typing.Any]:
    flags = cxx_flags_tset(actions, flag_deps)
    if flags == None:
        return []
    return [flags.project_as_args("linker_flags", ordering = "postorder")]

cxx_flags = prelude_rule(
    name = "cxx_flags",
    impl = cxx_flags_impl,
    docs = """
        A `cxx_flags()` target holds flags that C++ targets pull in through their
        `flags` attribute. Its `deps` can reference other `cxx_flags`
        targets; nested flags apply before the including target's flags, and all
        shared flags apply before the consuming C++ target's own flags.

        Consumers share a compiler argsfile for the transitive flag set.

        Write-to-file macros (`$(@...)`) are unsupported in all flag attributes.
    """,
    examples = """
        ```
        cxx_flags(
          name = "common_warnings",
          compiler_flags = ["-Wall", "-Wextra"],
        )

        cxx_library(
          name = "lib",
          srcs = ["lib.cpp"],
          flags = [":common_warnings"],
        )
        ```
    """,
    further = None,
    attrs = (
        {
            "compiler_flags": attrs.list(attrs.arg(), default = []),
            "deps": attrs.list(attrs.dep(providers = [CxxFlagsInfo]), default = []),
            "lang_compiler_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "lang_preprocessor_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "preprocessor_flags": attrs.list(attrs.arg(), default = []),
        }
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
