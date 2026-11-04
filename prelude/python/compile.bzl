# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":manifest.bzl", "ManifestInfo")
load(":toolchain.bzl", "PythonToolchainInfo")

def compile_manifests(ctx: AnalysisContext, manifests: list[ManifestInfo], mode: str = "UNCHECKED_HASH") -> ManifestInfo:
    # packagers only resolve content-based paths under `bytecode_UNCHECKED_HASH`
    has_content_based_path = mode == "UNCHECKED_HASH" and ctx.attrs._python_toolchain[PythonToolchainInfo].supports_content_based_paths == True
    output = ctx.actions.declare_output("bytecode_{}".format(mode), dir = True, has_content_based_path = has_content_based_path)
    bytecode_manifest = ctx.actions.declare_output("bytecode_{}.manifest".format(mode), has_content_based_path = has_content_based_path)
    cmd = [
        ctx.attrs._python_toolchain[PythonToolchainInfo].host_interpreter,
        ctx.attrs._python_toolchain[PythonToolchainInfo].compile,
        cmd_args(output.as_output(), format = "--output={}"),
        cmd_args(bytecode_manifest.as_output(), format = "--bytecode-manifest={}"),
        "--invalidation-mode={}".format(mode),
    ]

    env = {
        # On some platforms (e.g. linux), python hash code randomness can cause
        # the bytecode to be non-deterministic, so pin via the `PYTHONHASHSEED`
        # env var.
        "PYTHONHASHSEED": "7",
    }

    # support invalidating cached pyc compile actions by bumping the env var.
    # the value itself is meaningless, just the fact it changes is meaningful.
    # using the PYC magic number for *convenience* only
    toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    python_version = toolchain.python_version
    is_python312 = python_version.version_tuple == (3, 12) if python_version else (toolchain.version and "3.12" in toolchain.version)
    if is_python312:
        # for CPython, the magic number *shouldn't* change during the lifetime of a feature release
        # but internally we do make more significant changes (rarely),
        # so for those cases we support forced invalidation using this env var
        env["PYTHON312_DUMMY_PYC_CACHE_BUSTER"] = "3532"

    hidden = []
    for manifest in manifests:
        cmd.append(manifest.manifest)
        hidden.extend([a for a, _ in manifest.artifacts])
    ctx.actions.run(
        cmd_args(cmd, hidden = hidden),
        env = env,
        category = "py_compile",
        identifier = mode,
        error_handler = ctx.attrs._python_toolchain[PythonToolchainInfo].python_error_handler,
    )
    return ManifestInfo(manifest = bytecode_manifest.without_associated_artifacts(), artifacts = [(output, "bytecode")])
