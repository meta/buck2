# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "CxxToolchainInfo",
    "LinkerType",
)

def _strip_debug_info(actions: AnalysisActions, cxx_toolchain: CxxToolchainInfo, out: str, obj: Artifact, has_content_based_path: bool) -> Artifact:
    """
    Strip debug information from an object.
    """
    strip = cxx_toolchain.binary_utilities_info.strip
    output = actions.declare_output("__stripped__", out, has_content_based_path = has_content_based_path)
    if cxx_toolchain.linker_info.type == LinkerType("gnu"):
        cmd = cmd_args([strip, "--strip-debug", "--strip-unneeded", "-o", output.as_output(), obj])
    else:
        cmd = cmd_args([strip, "-S", "-o", output.as_output(), obj])
    actions.run(cmd, category = "strip_debug", identifier = out)
    return output

_InterfaceInfo = provider(
    fields = {
        "artifact": provider_field(typing.Any, default = None),  # "artifact"
    }
)

def _anon_strip_debug_info_impl(ctx):
    output = _strip_debug_info(
        actions = ctx.actions,
        cxx_toolchain = ctx.attrs._cxx_toolchain[CxxToolchainInfo],
        out = ctx.attrs.out,
        obj = ctx.attrs.obj,
        has_content_based_path = ctx.attrs.has_content_based_path,
    )
    return [DefaultInfo(), _InterfaceInfo(artifact = output)]

# Anonymous wrapper for `extract_symbol_names`.
_anon_strip_debug_info = anon_rule(
    impl = _anon_strip_debug_info_impl,
    attrs = {
        "has_content_based_path": attrs.bool(),
        "obj": attrs.source(),
        "out": attrs.string(),
        "_cxx_toolchain": attrs.dep(providers = [CxxToolchainInfo]),
    },
    artifact_promise_mappings = {
        "strip_debug_info": lambda p: p[_InterfaceInfo].artifact,
    },
)

def strip_debug_info(
    actions: AnalysisActions,
    out: str,
    obj: Artifact,
    cxx_toolchain_info: CxxToolchainInfo | None = None,
    cxx_toolchain: Dependency | None = None,
    anonymous: bool = False,
    has_content_based_path: bool = False,
) -> Artifact:
    if anonymous:
        strip_debug_info = actions.anon_target(
            _anon_strip_debug_info,
            dict(
                _cxx_toolchain = cxx_toolchain,
                out = out,
                obj = obj,
                has_content_based_path = has_content_based_path,
            ),
        ).artifact("strip_debug_info")

        if has_content_based_path:
            return actions.assert_has_content_based_path(strip_debug_info)

        return actions.assert_short_path(strip_debug_info, short_path = out)
    else:
        return _strip_debug_info(
            actions = actions,
            cxx_toolchain = cxx_toolchain_info,
            out = out,
            obj = obj,
            has_content_based_path = has_content_based_path,
        )

def strip_object(
    ctx: AnalysisContext,
    cxx_toolchain: CxxToolchainInfo,
    unstripped: Artifact,
    strip_flags: cmd_args,
    category_suffix: [str, None] = None,
    output_path: [str, None] = None,
    allow_cache_upload: bool = False,
) -> Artifact:
    """
    Strip unneeded information from binaries / shared libs.
    """
    strip = cxx_toolchain.binary_utilities_info.strip

    output_path = output_path or unstripped.short_path
    stripped_lib = ctx.actions.declare_output("stripped/{}".format(output_path), has_content_based_path = False)

    # TODO(T109996375) support configuring the flags used for stripping
    cmd = cmd_args(
        strip,
        strip_flags,
        unstripped,
        "-o",
        stripped_lib.as_output(),
    )

    effective_category_suffix = category_suffix if category_suffix else "shared_lib"
    category = "strip_{}".format(effective_category_suffix)

    ctx.actions.run(cmd, category = category, identifier = output_path, allow_cache_upload = allow_cache_upload)

    return stripped_lib

# Sections from `-fpseudo-probe-for-profiling`. Only profile tools read them; the program
# does not load them at run time. `--strip-debug` does not remove them (it removes only
# `.debug*` sections), and `--only-keep-debug` copies them into the debuginfo binary.
_PSEUDO_PROBE_SECTIONS = [".pseudo_probe", ".pseudo_probe_desc"]

def strip_debug_with_gnu_debuglink(ctx: AnalysisContext, name: str, obj: Artifact) -> tuple:
    """
    Split a binary into a separate debuginfo binary and a stripped binary with a .gnu_debuglink reference.
    Pseudo-probe sections go into the debuginfo binary only.
    """
    objcopy = get_cxx_toolchain_info(ctx).binary_utilities_info.objcopy

    # We flatten the directory structure because .gnu_debuglink doesn't understand directories and we
    # need to avoid name conflicts between different inputs
    debuginfo_name = name.replace("/", ".")
    debuginfo_output = ctx.actions.declare_output("__debuginfo__", debuginfo_name + ".debuginfo", has_content_based_path = False)
    cmd = cmd_args([objcopy, "--only-keep-debug", obj, debuginfo_output.as_output()])
    ctx.actions.run(cmd, category = "extract_debuginfo", identifier = name, local_only = get_cxx_toolchain_info(ctx).linker_info.link_binaries_locally)

    binary_output = ctx.actions.declare_output("__stripped_objects__", name, has_content_based_path = False)
    cmd = cmd_args(
        objcopy,
        "--strip-debug",
        "--keep-file-symbols",
        ["--remove-section=" + section for section in _PSEUDO_PROBE_SECTIONS],
        "--add-gnu-debuglink",
        debuginfo_output,
        obj,
        binary_output.as_output(),
    )
    ctx.actions.run(cmd, category = "strip_debug", identifier = name, local_only = get_cxx_toolchain_info(ctx).linker_info.link_binaries_locally)

    return binary_output, debuginfo_output
