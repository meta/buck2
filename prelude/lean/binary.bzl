# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""`lean_binary` and `lean_test`: Lean executables.

They compile their sources like `lean_library`, and link the objects, the
dependencies and the toolchain runtime into an executable with the cxx
toolchain. One of the modules must define `main`.
"""

load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//cxx:link.bzl", "cxx_link_into")
load("@prelude//cxx:link_types.bzl", "CxxLinkResultType", "link_options")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference")
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",
    "LinkStrategy",
    "get_link_args_for_strategy",
)
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//tests:re_utils.bzl", "get_re_executors_from_props")
load(":library.bzl", "compile_lean_library", "lean_library_attrs")
load(":providers.bzl", "LeanToolchainInfo")
load(":transition.bzl", "lean_transition")

def _executable_name(ctx: AnalysisContext, cxx: CxxToolchainInfo) -> str:
    """The output name: the target name, with the extension of the cxx
    toolchain, or `.exe` on Windows when the toolchain sets none."""
    extension = cxx.linker_info.binary_extension
    if not extension and ctx.attrs._target_os_type[OsLookup].os == Os("windows"):
        extension = "exe"
    return ctx.label.name + ("." + extension if extension else "")

def _link_executable(ctx: AnalysisContext) -> Artifact:
    library = compile_lean_library(ctx, link_whole = False)
    cxx = get_cxx_toolchain_info(ctx)
    toolchain = ctx.attrs.lean_toolchain[LeanToolchainInfo]
    link_args = get_link_args_for_strategy(
        ctx.actions,
        ctx.label,
        cxx.linker_info,
        [library.merged_link_info],
        LinkStrategy("static_pic"),
        prefer_stripped = False,
        transformation_spec_context = None,
    )
    result = cxx_link_into(
        ctx = ctx,
        output = ctx.actions.declare_output(_executable_name(ctx, cxx), has_content_based_path = False),
        result_type = CxxLinkResultType("executable"),
        opts = link_options(
            links = [
                LinkArgs(flags = cmd_args(cxx.linker_info.binary_linker_flags, toolchain.linker_flags, ctx.attrs.linker_flags)),
                link_args,
            ],
            link_execution_preference = LinkExecutionPreference("any"),
            category_suffix = "lean_binary",
        ),
    )
    return result.linked_object.output

def _lean_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    exe = _link_executable(ctx)
    return [
        DefaultInfo(default_output = exe),
        RunInfo(args = cmd_args(exe)),
    ]

def _lean_test_impl(ctx: AnalysisContext) -> list[Provider]:
    exe = _link_executable(ctx)
    command = [cmd_args(exe)] + ctx.attrs.args
    re_executors = get_re_executors_from_props(ctx)
    return [
        DefaultInfo(default_output = exe),
        RunInfo(args = cmd_args(command)),
        ExternalRunnerTestInfo(
            type = "custom",
            command = command,
            env = ctx.attrs.env,
            labels = ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            default_executor = re_executors.default_executor,
            executor_overrides = re_executors.executor_overrides,
            run_from_project_root = re_executors.run_from_project_root,
            use_project_relative_paths = re_executors.use_project_relative_paths,
            supports_test_execution_caching = ctx.attrs.supports_test_execution_caching,
        ),
    ]

_binary_attrs = lean_library_attrs | {
    # Read by `cxx_link_into`.
    "bolt_flags": attrs.list(attrs.arg(), default = []),
    "bolt_profile": attrs.option(attrs.source(), default = None),
    "deffile": attrs.option(attrs.source(), default = None),
    "linker_flags": attrs.list(attrs.arg(), default = [], doc = "Extra linker flags."),
    "strip_stapsdt": attrs.bool(default = False),
    "_target_os_type": buck.target_os_type_arg(),
}

lean_binary = rule(
    impl = _lean_binary_impl,
    attrs = _binary_attrs,
    cfg = lean_transition,
)

lean_test = rule(
    impl = _lean_test_impl,
    attrs = _binary_attrs
    | {
        "args": attrs.list(attrs.arg(), default = [], doc = "Arguments for the test binary."),
        "contacts": attrs.list(attrs.string(), default = []),
        "env": attrs.dict(key = attrs.string(), value = attrs.arg(), sorted = False, default = {}, doc = "Environment of the test binary."),
        "supports_test_execution_caching": attrs.bool(default = True),
    }
    | re_test_common.test_args(),
    cfg = lean_transition,
)
