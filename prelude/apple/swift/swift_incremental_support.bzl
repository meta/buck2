# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//apple/swift:swift_toolchain_types.bzl",
    "SwiftObjectFormat",
    "SwiftToolchainInfo",  # @unused Used as a type
)
load("@prelude//apple/swift:swift_types.bzl", "SwiftCompilationModes")
load(
    "@prelude//cxx:cxx_sources.bzl",
    "CxxSrcWithFlags",  # @unused Used as a type
)
load(":swift_toolchain.bzl", "get_swift_toolchain_info", "include_path_for_relative_module_map_paths")

_OutputFileMapData = record(
    artifacts = field(list[Artifact]),
    outputs = field(list[Artifact]),
    output_file_map = field(dict),
    swiftdeps = field(list[Artifact]),
    depfiles = field(list[Artifact]),
)

IncrementalCompilationOutput = record(
    artifacts = field(list[Artifact]),
    incremental_flags_cmd = field(cmd_args),
    num_threads = field(int),
    output_file_map = field(dict),
    skip_incremental_outputs = field(bool),
    incremental_remote_outputs = field(bool),
    swiftdeps = field(list[Artifact]),
    depfiles = field(list[Artifact]),
    swiftdoc = field(Artifact | None),
)

IncrementalCompilationInput = record(
    swiftdeps = field(list[Artifact]),
    depfiles = field(list[Artifact]),
    swiftdoc = field(Artifact | None),
)

SwiftmoduleIncrementalCompilationOutput = record(
    # Driver flags specific to the incremental swiftmodule action. The caller
    # composes this with the shared emit-module / emit-objc-header / include-
    # path / skip-function-bodies flags it built itself.
    incremental_flags_cmd = field(cmd_args),
    output_file_map = field(dict),
    skip_incremental_outputs = field(bool),
    swiftdeps = field(list[Artifact]),
    depfiles = field(list[Artifact]),
)

SwiftCompilationMode = enum(*SwiftCompilationModes)

# The maxmium number of threads, we don't use
# host_info to prevent cache misses across
# different hardware models.
INCREMENTAL_SWIFT_COMPILE_MAX_NUM_THREADS = 6

# This is the default, but specifying it explicitly
# is clearer.
INCREMENTAL_SWIFT_COMPILE_BATCH_SIZE = 25

def should_build_swift_incrementally(ctx: AnalysisContext) -> bool:
    toolchain = get_swift_toolchain_info(ctx)

    # Incremental builds are only supported when object files are generated.
    if toolchain.object_format != SwiftObjectFormat("object"):
        return False

    return SwiftCompilationMode(ctx.attrs.swift_compilation_mode) == SwiftCompilationMode("incremental")

def get_incremental_object_compilation_flags(
    ctx: AnalysisContext, srcs: list[CxxSrcWithFlags], output_swiftmodule: Artifact, output_swiftdoc: Artifact | None, output_header: Artifact
) -> IncrementalCompilationOutput:
    output_file_map_data = _get_output_file_map(ctx, srcs)
    return _get_incremental_compilation_flags_and_objects(
        ctx,
        output_file_map_data,
        output_swiftmodule,
        output_swiftdoc,
        output_header,
        len(srcs),
    )

def get_incremental_swiftmodule_compilation_flags(ctx: AnalysisContext, srcs: list[CxxSrcWithFlags]) -> SwiftmoduleIncrementalCompilationOutput:
    """Returns the incremental-driver-only flags + per-target incremental
    state (output file map, swiftdeps) for the incremental swiftmodule
    action. The shared emit-module / emit-objc-header / include-path /
    skip-function-bodies flags are intentionally NOT included here — the
    caller (\`_compile_swiftmodule\`) builds them once and shares them with
    the WMO path.
    """
    output_file_map_data = _get_swiftmodule_output_file_map(ctx, srcs)

    # The emit-module-separately job is a single driver task, so we don't
    # pass `-j` or `-driver-batch-size-limit` — the driver picks defaults.
    cmd = cmd_args(
        [
            "-enable-batch-mode",
            "-enable-incremental-imports",
            "-experimental-emit-module-separately",
            "-incremental",
        ],
        hidden = [output.as_output() for output in output_file_map_data.outputs],
    )

    skip_incremental_outputs = _get_skip_swift_incremental_outputs(ctx)
    uses_content_based_paths = get_uses_content_based_paths(ctx)
    if get_swift_incremental_logging_enabled(ctx):
        cmd.add([
            "-driver-show-incremental",
            "-driver-show-job-lifecycle",
        ])

    if skip_incremental_outputs:
        # When skipping incremental outputs, we write the contents of the
        # output_file_map in the swift wrapper and need to ensure this is
        # an output file (vs being an input in normal cases)
        output_map_artifact = ctx.actions.declare_output("__swift_module_incremental__/output_file_map.json", has_content_based_path = uses_content_based_paths)
        cmd.add(
            "-Xwrapper",
            "-skip-incremental-outputs",
            "-output-file-map",
            output_map_artifact.as_output(),
        )
    elif get_incremental_file_hashing_enabled(ctx):
        cmd.add(
            "-enable-incremental-file-hashing",
            "-avoid-emit-module-source-info",
        )
    if get_incremental_remote_outputs_enabled(ctx):
        cmd.add(
            "-Xwrapper",
            "--no-file-prefix-map",
            "-dwarf-version=5",
            "-Xcc",
            "-working-directory",
            "-Xcc",
            ".",
        )

    return SwiftmoduleIncrementalCompilationOutput(
        incremental_flags_cmd = cmd,
        output_file_map = output_file_map_data.output_file_map,
        skip_incremental_outputs = skip_incremental_outputs,
        swiftdeps = output_file_map_data.swiftdeps,
        depfiles = output_file_map_data.depfiles,
    )

def _get_incremental_num_threads(num_srcs: int) -> int:
    if num_srcs == 0:
        return 1

    src_threads = (num_srcs + INCREMENTAL_SWIFT_COMPILE_BATCH_SIZE - 1) // INCREMENTAL_SWIFT_COMPILE_BATCH_SIZE
    return min(INCREMENTAL_SWIFT_COMPILE_MAX_NUM_THREADS, src_threads)

def _get_skip_swift_incremental_outputs(ctx: AnalysisContext):
    return getattr(ctx.attrs, "_skip_swift_incremental_outputs", False)

def get_incremental_file_hashing_enabled(ctx: AnalysisContext):
    toolchain = get_swift_toolchain_info(ctx)
    return toolchain.supports_incremental_file_hashing and getattr(ctx.attrs, "swift_incremental_file_hashing", False) and should_build_swift_incrementally(ctx)

def get_swift_incremental_logging_enabled(ctx: AnalysisContext):
    return getattr(ctx.attrs, "swift_incremental_logging", False)

def get_incremental_split_actions(ctx: AnalysisContext):
    return getattr(ctx.attrs, "_swift_incremental_split_actions", False)

def get_incremental_swiftmodule_action(ctx: AnalysisContext):
    return getattr(ctx.attrs, "_swift_incremental_swiftmodule_action", False)

def should_build_swiftmodule_incrementally(ctx: AnalysisContext) -> bool:
    # The new incremental swiftmodule action requires:
    #  * Swift built incrementally
    #  * Split actions on (the object action is what we split from)
    #  * The new opt-in attribute on
    return should_build_swift_incrementally(ctx) and get_incremental_split_actions(ctx) and get_incremental_swiftmodule_action(ctx)

def get_incremental_remote_outputs_enabled(ctx: AnalysisContext):
    return getattr(ctx.attrs, "incremental_remote_outputs", False)

def get_uses_content_based_paths(ctx, toolchain: SwiftToolchainInfo | None = None):
    if getattr(ctx.attrs, "has_content_based_path", None) != None:
        return ctx.attrs.has_content_based_path

    toolchain = toolchain or get_swift_toolchain_info(ctx)
    return toolchain.uses_content_based_paths

def _get_incremental_compilation_flags_and_objects(
    ctx: AnalysisContext,
    output_file_map_data: _OutputFileMapData,
    output_swiftmodule: Artifact,
    output_swiftdoc: Artifact | None,
    output_header: Artifact,
    num_srcs: int,
) -> IncrementalCompilationOutput:
    extra_hidden = [output_swiftdoc.as_output()] if output_swiftdoc else []

    # Match the driver -j to the Buck `weight` (num_threads) so the frontend-thread count matches
    # the permits Buck reserves. Otherwise -j was always 6 while weight could be 1, oversubscribing
    # up to ~6x and under-counting CPU in the (weight-based) low-pass-filter capacity.
    num_threads = _get_incremental_num_threads(num_srcs)
    cmd = cmd_args(
        [
            "-disable-cmo",
            "-emit-object",
            "-enable-batch-mode",
            "-enable-incremental-imports",
            "-experimental-emit-module-separately",
            "-incremental",
            "-j",
            str(num_threads),
            "-driver-batch-size-limit",
            str(INCREMENTAL_SWIFT_COMPILE_BATCH_SIZE),
        ],
        hidden = [output.as_output() for output in output_file_map_data.outputs] + extra_hidden,
    )
    cmd.add(include_path_for_relative_module_map_paths(ctx))

    if not get_incremental_split_actions(ctx):
        cmd.add(
            [
                "-emit-objc-header",
                "-emit-objc-header-path",
                output_header.as_output(),
                "-emit-module",
                "-emit-module-path",
                output_swiftmodule.as_output(),
            ],
        )

    skip_incremental_outputs = _get_skip_swift_incremental_outputs(ctx)
    incremental_remote_outputs = False
    uses_content_based_paths = get_uses_content_based_paths(ctx)
    if get_swift_incremental_logging_enabled(ctx):
        cmd.add([
            "-driver-show-incremental",
            "-driver-show-job-lifecycle",
        ])

    if skip_incremental_outputs:
        # When skipping incremental outputs, we write the contents of the
        # output_file_map in the swift wrapper and need to ensure this is
        # an output file (vs being an input in normal cases)
        output_map_artifact = ctx.actions.declare_output("__swift_incremental__/output_file_map.json", has_content_based_path = uses_content_based_paths)
        cmd.add(
            "-Xwrapper",
            "-skip-incremental-outputs",
            "-output-file-map",
            output_map_artifact.as_output(),
        )
    elif get_incremental_file_hashing_enabled(ctx):
        cmd.add(
            "-enable-incremental-file-hashing",
            "-avoid-emit-module-source-info",
        )
    if get_incremental_remote_outputs_enabled(ctx):
        incremental_remote_outputs = True
        cmd.add(
            "-Xwrapper",
            "--no-file-prefix-map",
            "-dwarf-version=5",
            "-Xcc",
            "-working-directory",
            "-Xcc",
            ".",
        )

    return IncrementalCompilationOutput(
        artifacts = output_file_map_data.artifacts,
        incremental_flags_cmd = cmd,
        num_threads = num_threads,
        output_file_map = output_file_map_data.output_file_map,
        skip_incremental_outputs = skip_incremental_outputs,
        swiftdeps = output_file_map_data.swiftdeps,
        depfiles = output_file_map_data.depfiles,
        incremental_remote_outputs = incremental_remote_outputs,
        swiftdoc = output_swiftdoc,
    )

def _get_output_file_map(ctx: AnalysisContext, srcs: list[CxxSrcWithFlags]) -> _OutputFileMapData:
    uses_content_based_paths = get_uses_content_based_paths(ctx)
    if _get_skip_swift_incremental_outputs(ctx):
        all_outputs = []
        swiftdeps = []
        artifacts = []
        depfiles = []
        output_file_map = {}

        for src in srcs:
            file_name = src.file.basename
            output_artifact = ctx.actions.declare_output("__swift_incremental__/objects/" + file_name + ".o", has_content_based_path = uses_content_based_paths)
            artifacts.append(output_artifact)
            all_outputs.append(output_artifact)
    else:
        # swift-driver doesn't respect extension for root swiftdeps file and it always has to be `.priors`.
        module_swiftdeps = ctx.actions.declare_output(
            "__swift_incremental__/swiftdeps/module-build-record.priors", has_content_based_path = uses_content_based_paths
        )
        output_file_map = {
            "": {
                "swift-dependencies": module_swiftdeps,
            },
        }
        all_outputs = [module_swiftdeps]
        swiftdeps = [module_swiftdeps]
        artifacts = []
        depfiles = []
        toolchain = get_swift_toolchain_info(ctx)

        for src in srcs:
            file_name = src.file.basename
            output_artifact = ctx.actions.declare_output("__swift_incremental__/objects/" + file_name + ".o", has_content_based_path = uses_content_based_paths)
            artifacts.append(output_artifact)
            all_outputs.append(output_artifact)
            swiftdeps_artifact = ctx.actions.declare_output(
                "__swift_incremental__/swiftdeps/" + file_name + ".swiftdeps", has_content_based_path = uses_content_based_paths
            )
            output_file_map[src.file] = {
                "object": output_artifact,
                "swift-dependencies": swiftdeps_artifact,
            }
            swiftdeps.append(swiftdeps_artifact)
            all_outputs.append(swiftdeps_artifact)
            if toolchain.use_depsfiles and not get_incremental_file_hashing_enabled(ctx) and not get_incremental_split_actions(ctx):
                deps_artifact = ctx.actions.declare_output("__swift_incremental__/objects/" + file_name + ".d", has_content_based_path = uses_content_based_paths)
                depfiles.append(deps_artifact)
                all_outputs.append(deps_artifact)

    return _OutputFileMapData(
        artifacts = artifacts,
        outputs = all_outputs,
        output_file_map = output_file_map,
        swiftdeps = swiftdeps,
        depfiles = depfiles,
    )

def _get_swiftmodule_output_file_map(ctx: AnalysisContext, srcs: list[CxxSrcWithFlags]) -> _OutputFileMapData:
    uses_content_based_paths = get_uses_content_based_paths(ctx)
    if _get_skip_swift_incremental_outputs(ctx):
        return _OutputFileMapData(
            artifacts = [],
            outputs = [],
            output_file_map = {},
            swiftdeps = [],
            depfiles = [],
        )

    module_swiftdeps = ctx.actions.declare_output(
        "__swift_module_incremental__/swiftdeps/module-build-record.priors", has_content_based_path = uses_content_based_paths
    )
    output_file_map = {
        "": {
            "swift-dependencies": module_swiftdeps,
        },
    }

    # For incremental swiftmodule generation, we need to capture the set of
    # incremental output files they produce.
    per_source_dir = ctx.actions.declare_output(
        "__swift_module_incremental__/per_source_incremental", dir = True, has_content_based_path = uses_content_based_paths
    )
    for src in srcs:
        file_name = src.file.basename
        output_file_map[src.file] = {
            "diagnostics": cmd_args(per_source_dir, format = "{}/" + file_name + ".dia", delimiter = ""),
            "swift-dependencies": cmd_args(per_source_dir, format = "{}/" + file_name + ".swiftdeps", delimiter = ""),
            "swiftmodule": cmd_args(per_source_dir, format = "{}/" + file_name + ".swiftmodule", delimiter = ""),
        }

    return _OutputFileMapData(
        artifacts = [],
        outputs = [module_swiftdeps, per_source_dir],
        output_file_map = output_file_map,
        swiftdeps = [module_swiftdeps, per_source_dir],
        depfiles = [],
    )
