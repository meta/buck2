# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")

# BOLT-pattern suffix. Wrapper writes the canonical-named patched binary
# in the same directory to keep `$ORIGIN`-relative RPATH valid.
PRE_EXTRACT_SUFFIX = "-pre_extract"

HipDebugExtractOutputs = record(
    patched_output = field(Artifact),
    # arch label (e.g. "gfx942") -> sidecar .debug
    arch_debug_files = field(dict[str, Artifact]),
)

def hip_debug_extract_available(toolchain: CxxToolchainInfo) -> bool:
    return toolchain.hip_device_debug_extract and bool(toolchain.hip_gpu_archs) and toolchain.hip_debug_extract != None

# Per-TU compile-time strip: runs the wrapper on a single `.o`'s one
# bundle, emitting per-arch sidecars. The host linker concatenates
# stripped bundles into the final `.hip_fatbin`; per-TU
# `.hipFatBinSegment` pointers are resolved by linker relocation.
def hip_debug_extract_compile_object(
    actions: AnalysisActions, toolchain: CxxToolchainInfo, obj: Artifact, identifier: [str, None] = None
) -> HipDebugExtractOutputs:
    archs = toolchain.hip_gpu_archs

    stripped_name = obj.short_path + ".stripped.o"
    stripped_output = actions.declare_output(stripped_name, has_content_based_path = True)
    canonical = obj.short_path
    arch_debug_files = {arch: actions.declare_output("{}.{}.debug".format(canonical, arch)) for arch in archs}

    cmd = cmd_args([
        toolchain.hip_debug_extract,
        cmd_args(obj, format = "--input-binary={}"),
        cmd_args(stripped_output.as_output(), format = "--output-binary={}"),
    ])
    for arch, debug_out in arch_debug_files.items():
        cmd.add(
            cmd_args(
                debug_out.as_output(),
                format = "--arch-debug-output=" + arch + "={}",
            )
        )

    actions.run(
        cmd,
        category = "hip_debug_extract_tu",
        identifier = identifier,
    )

    return HipDebugExtractOutputs(
        patched_output = stripped_output,
        arch_debug_files = arch_debug_files,
    )
