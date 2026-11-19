# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",  # @unused Used as a type
)
load(":swift_toolchain_types.bzl", "SwiftCompiledModuleTset")

SWIFT_EXTENSION = ".swift"

SWIFTMODULE_EXTENSION = ".swiftmodule"

# Base action categories for Swift compiles. When explicit modules are enabled,
# EXPLICIT_MODULES_CATEGORY_SUFFIX is appended (see swift_compilation.bzl).
SWIFT_COMPILE_CATEGORY = "swift_compile"

SWIFTMODULE_COMPILE_CATEGORY = "swiftmodule_compile"

EXPLICIT_MODULES_CATEGORY_SUFFIX = "_with_explicit_mods"

SwiftCompilationModes = ("wmo", "incremental", "auto")

SwiftMacroPlugin = plugins.kind()

SwiftVersion = ("5", "6")

SwiftDependencyInfo = provider(
    fields = {
        "debug_info_tset": provider_field(ArtifactTSet),
        # Includes modules through exported_deps, used for compilation
        "exported_swiftmodules": provider_field(SwiftCompiledModuleTset),
        # If this target has exported_headers, used to validate non-modular dep exports
        "has_exported_headers": provider_field(bool),
        # Macro deps cannot be mixed with apple_library deps
        "is_macro": provider_field(bool),
        # If this target provides a clang module, used to validate non-modular dep exports
        "is_modular": provider_field(bool),
        # Tset of this target's own `-dump-ast` output directory (see the `[ast]`
        # subtarget) merged with the same tset from all deps.
        "swift_ast_dump_tset": provider_field(ArtifactTSet),
        # Tset of this target's own `.swiftmodule` output merged with the
        # same tsets from all deps.
        "swiftmodule_change_analysis_tset": provider_field(ArtifactTSet),
    }
)

def _swift_framework_implicit_search_paths_args(args: cmd_args):
    return args

FrameworkImplicitSearchPathInfoTSet = transitive_set(
    args_projections = {
        "swift_framework_implicit_search_paths_args": _swift_framework_implicit_search_paths_args,
    },
)

FrameworkImplicitSearchPathInfo = provider(
    fields = {
        "tset": provider_field(typing.Any, default = None),  # A tset of FrameworkImplicitSearchPathInfoTSet
    }
)

def get_implicit_framework_search_path_providers(ctx: AnalysisContext, value: [cmd_args, None], deps: list[Dependency]) -> FrameworkImplicitSearchPathInfoTSet:
    deps_infos = [
        d[FrameworkImplicitSearchPathInfo].tset for d in deps if FrameworkImplicitSearchPathInfo in d and d[FrameworkImplicitSearchPathInfo].tset != None
    ]
    if value:
        return ctx.actions.tset(FrameworkImplicitSearchPathInfoTSet, value = value, children = deps_infos)
    else:
        return ctx.actions.tset(FrameworkImplicitSearchPathInfoTSet, children = deps_infos)
