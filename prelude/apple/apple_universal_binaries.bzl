# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifact_tset.bzl", "make_artifact_tset")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//linking:link_info.bzl", "UnstrippedLinkOutputInfo")
load(":apple_bundle_types.bzl", "AppleBundleBinaryOutput")
load(":apple_toolchain_types.bzl", "AppleToolsInfo")
load(":debug.bzl", "AppleDebuggableInfo")

def get_universal_binary_name(ctx: AnalysisContext) -> str:
    if ctx.attrs.executable_name:
        return ctx.attrs.executable_name
    binary_deps = ctx.attrs.executable

    # Because `binary_deps` is a split transition of the same target,
    # the filenames would be identical, so we just pick the first one.
    first_binary_dep = binary_deps.values()[0]
    first_binary_artifact = first_binary_dep[DefaultInfo].default_outputs[0]

    # The universal executable should have the same name as the base/thin ones
    return first_binary_artifact.short_path

def lipo_binary_artifacts(ctx: AnalysisContext, binaries: list[Artifact], binary_name: [str, None], lipo: RunInfo, identifier: str) -> Artifact:
    binary_output = ctx.actions.declare_output("UniversalBinary" if binary_name == None else binary_name, dir = False, has_content_based_path = False)
    lipo_cmd = [lipo] + binaries

    lipo_cmd.extend(["-create", "-output", binary_output.as_output()])
    ctx.actions.run(cmd_args(lipo_cmd), category = "lipo", identifier = identifier)

    return binary_output

def lipo_binaries(ctx: AnalysisContext, binary_deps: dict[str, Dependency], binary_name: [str, None], lipo: RunInfo) -> Artifact:
    binaries = [binary[DefaultInfo].default_outputs[0] for binary in binary_deps.values()]
    return lipo_binary_artifacts(ctx, binaries, binary_name, lipo, "default")

def _get_unstripped_binaries(binary_deps: dict[str, Dependency]) -> list[Artifact]:
    unstripped_binaries = []
    for binary_dep in binary_deps.values():
        default_binary = binary_dep[DefaultInfo].default_outputs[0]
        unstripped_binary = binary_dep.get(UnstrippedLinkOutputInfo).artifact if binary_dep.get(UnstrippedLinkOutputInfo) != None else None
        if unstripped_binary != None and unstripped_binary != default_binary:
            unstripped_binaries.append(unstripped_binary)

    if len(unstripped_binaries) > 0 and len(unstripped_binaries) != len(binary_deps):
        fail("Inconsistent unstripped binaries, only certain archs provided unstripped binary versions")

    return unstripped_binaries

def create_universal_binary(
    ctx: AnalysisContext, binary_deps: dict[str, Dependency], binary_name: [str, None], dsym_bundle_name: [str, None], split_arch_dsym: bool
) -> AppleBundleBinaryOutput:
    binary_output = lipo_binaries(ctx, binary_deps, binary_name, ctx.attrs._apple_toolchain[AppleToolchainInfo].lipo)

    unstripped_binaries = _get_unstripped_binaries(binary_deps)
    unstripped_binary_output = None
    if unstripped_binaries:
        unstripped_binary_name = "unstripped/{}".format(binary_name) if binary_name else "UniversalBinary-Unstripped"
        unstripped_binary_output = lipo_binary_artifacts(
            ctx, unstripped_binaries, unstripped_binary_name, ctx.attrs._apple_toolchain[AppleToolchainInfo].lipo, "unstripped"
        )

    # Universal binaries can be created out of plain `cxx_binary()` / `cxx_library()`
    # which lack the `AppleDebuggableInfo` provider.
    # TODO(T174234334): Uniformly support debuggable info for apple_*/cxx_*
    contains_full_debuggable_info = _all_binaries_have_apple_debuggable_info(binary_deps)

    dsym_output = None
    if split_arch_dsym and contains_full_debuggable_info:
        dsym_output = ctx.actions.declare_output(
            "UniversalBinary.dSYM" if dsym_bundle_name == None else dsym_bundle_name, dir = True, has_content_based_path = False
        )
        dsym_combine_cmd = [ctx.attrs._apple_tools[AppleToolsInfo].split_arch_combine_dsym_bundles_tool]

        for arch, binary in binary_deps.items():
            dsym_combine_cmd.extend(["--dsym-bundle", cmd_args(binary.get(AppleDebuggableInfo).dsyms[0]), "--arch", arch])
        dsym_combine_cmd.extend(["--output", dsym_output.as_output()])
        ctx.actions.run(cmd_args(dsym_combine_cmd), category = "universal_binaries_dsym")

    all_debug_info_tsets = []
    if contains_full_debuggable_info:
        all_debug_info_tsets = [binary.get(AppleDebuggableInfo).debug_info_tset for binary in binary_deps.values()]

    return AppleBundleBinaryOutput(
        binary = binary_output,
        unstripped_binary = unstripped_binary_output,
        debuggable_info = AppleDebuggableInfo(
            dsyms = [dsym_output] if dsym_output != None else [],
            binaries = [unstripped_binary_output] if unstripped_binary_output else [binary_output],
            debug_info_tset = make_artifact_tset(
                actions = ctx.actions,
                label = ctx.label,
                children = filter(None, all_debug_info_tsets),
            ),
        ),
    )

def _all_binaries_have_apple_debuggable_info(binary_deps: dict[str, Dependency]) -> bool:
    for binary in binary_deps.values():
        info = binary.get(AppleDebuggableInfo)
        if info == None:
            return False
    return True
