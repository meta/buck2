# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused Used as a type
load(":apple_toolchain_types.bzl", "AppleToolchainInfo")
load(":debug.bzl", "AppleDebuggableInfo", "AppleSelectiveDebuggableMetadata")  # @unused Used as a type

DSYM_SUBTARGET = "dsym"
DSYM_INFO_SUBTARGET = "dsym-info"
EXTENDED_DSYM_INFO_SUBTARGET = "extended-dsym-info"
DWARF_AND_DSYM_SUBTARGET = "dwarf-and-dsym"

def get_apple_dsym(
    ctx: AnalysisContext,
    executable: Artifact,
    debug_info: list[ArgLike],
    action_identifier: str,
    output_path_override: [str, None] = None,
    dsym_allowlist: [Artifact, None] = None,
) -> Artifact:
    output_path = output_path_override or "{}.dSYM".format(executable.short_path)
    return get_apple_dsym_ext(ctx, executable, debug_info, action_identifier, output_path, dsym_allowlist)

# TODO(T110672942): Things which are still unsupported:
# - oso_prefix
#
# `dsym_allowlist` is an optional file in the format accepted by
# `dsymutil --allow`: only the debug info of the objects it lists is linked.
def get_apple_dsym_ext(
    ctx: AnalysisContext,
    executable: [ArgLike, Artifact],
    debug_info: list[ArgLike],
    action_identifier: str,
    output_path: str,
    dsym_allowlist: [Artifact, None] = None,
) -> Artifact:
    dsymutil = ctx.attrs._apple_toolchain[AppleToolchainInfo].dsymutil
    output = ctx.actions.declare_output(output_path, dir = True, has_content_based_path = False)
    cmd = cmd_args(
        [
            dsymutil,
            # Reproducers are not useful, we can reproduce from the action digest.
            "--reproducer=Off",
        ],
        # Mach-O executables don't contain DWARF data.
        # Instead, they contain paths to the object files which themselves contain DWARF data.
        # So, those object files are needed for dsymutil to be to create the dSYM bundle.
        hidden = debug_info,
    )
    if dsym_allowlist != None:
        # The separate form makes a `dsymutil` without `--allow` fail instead
        # of silently linking all debug info.
        cmd.add("--allow", dsym_allowlist)
    cmd.add(
        "-o",
        output.as_output(),
        executable,
    )
    ctx.actions.run(cmd, category = "apple_dsym", identifier = action_identifier)
    return output

AppleDsymJsonInfo = record(
    # JSON object containing the list of dSYMs
    json_object = field(dict[str, typing.Any]),
    # A list of all artifacts referenced in `json_object`
    outputs = field(list[Artifact]),
)

def get_apple_dsym_info_json(
    binary_dsyms: list[Artifact], dep_dsyms: list[Artifact], metadata: list[AppleSelectiveDebuggableMetadata] | None = None
) -> AppleDsymJsonInfo:
    dsym_info = {}

    if len(binary_dsyms) == 1:
        dsym_info["binary"] = binary_dsyms[0]
    else:
        fail("There can only be one binary dSYM")

    if dep_dsyms:
        # `dedupe` needed as it's possible for the same dSYM to bubble up
        # through multiple paths in a graph (e.g., including both a binary
        # + bundle in the `deps` field of a parent bundle).
        dsym_info["deps"] = dedupe(dep_dsyms)

    metadata_dsym_outputs = []
    if metadata != None:
        metadata_json_obj = {}
        for debuggable_metadata in metadata:
            metadata_json_obj[debuggable_metadata.dsym] = debuggable_metadata.metadata
            metadata_dsym_outputs += [debuggable_metadata.dsym, debuggable_metadata.metadata]
        dsym_info["selective_metadata"] = metadata_json_obj

    return AppleDsymJsonInfo(
        json_object = dsym_info,
        outputs = binary_dsyms + dep_dsyms + metadata_dsym_outputs,
    )

def get_deps_debuggable_infos(ctx: AnalysisContext) -> list[AppleDebuggableInfo]:
    binary_labels = filter(None, [getattr(binary_dep, "label", None) for binary_dep in (ctx.attrs.binary.values() if ctx.attrs.binary else {})])
    deps_debuggable_infos = filter(
        None,
        # It's allowed for `ctx.attrs.binary` to appear in `ctx.attrs.deps` as well,
        # in this case, do not duplicate the debugging info for the binary coming from two paths.
        [dep.get(AppleDebuggableInfo) for dep in ctx.attrs.deps if dep.label not in binary_labels],
    )
    return deps_debuggable_infos
