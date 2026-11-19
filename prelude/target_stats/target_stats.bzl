# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Shared implementation for the target_stats integration.

load("@prelude//:paths.bzl", "paths")
load(":target_stats_config.bzl", "TARGET_STATS_ENABLED")
load(":target_stats_tools.bzl", "TargetStatsToolsInfo")
load(
    ":target_stats_types.bzl",
    "TargetStatsInfo",
    "TargetStatsInfoTSet",
    "TargetStatsRecord",
)

CycleMode = enum("file", "package", "none")

_OUT_DIR = "__target_stats__"

def _content_based_input(ctx: AnalysisContext, artifact: Artifact, name: str) -> Artifact:
    """Return an input whose path can participate in cross-config dedupe.

    Source files are already eligible. Generated files are not necessarily
    content-addressed, so normalize those through a cheap copy before feeding
    them to the target_stats tools.
    """
    if artifact.is_source:
        return artifact
    return ctx.actions.copy_file(
        _OUT_DIR + "/inputs/" + name,
        artifact,
        has_content_based_path = True,
    )

def _file_stats_action(ctx: AnalysisContext, tools: TargetStatsToolsInfo, src: Artifact, name: str) -> Artifact:
    out = ctx.actions.declare_output(_OUT_DIR, name + ".file_stats.json", has_content_based_path = True)
    content_based_src = _content_based_input(ctx, src, name)
    ctx.actions.run(
        cmd_args([tools.file_stats, "--input", content_based_src, "--output", out.as_output()]),
        category = "target_stats_file_stats",
        identifier = name,
        allow_cache_upload = True,
        expect_eligible_for_dedupe = True,
    )
    return out

def _extract_target_data_action(ctx: AnalysisContext, tools: TargetStatsToolsInfo, file_stats_by_path: dict[str, Artifact]) -> Artifact:
    manifest = ctx.actions.write_json(
        _OUT_DIR + "/extract_manifest.json",
        file_stats_by_path,
        with_inputs = True,
        has_content_based_path = True,
    )
    out = ctx.actions.declare_output(_OUT_DIR, "target_data.json", has_content_based_path = True)
    ctx.actions.run(
        cmd_args([tools.extract_target_data, "--manifest", manifest, "--output", out.as_output()]),
        category = "target_stats_extract_target_data",
        allow_cache_upload = True,
        expect_eligible_for_dedupe = True,
    )
    return out

def _cycles_action(
    ctx: AnalysisContext, tools: TargetStatsToolsInfo, cycle_mode: CycleMode, target_data: Artifact, module_name: str, swift_dot: Artifact | None
) -> Artifact | None:
    if cycle_mode == CycleMode("none"):
        return None
    out = ctx.actions.declare_output(_OUT_DIR, "cycles.json", has_content_based_path = True)
    if cycle_mode == CycleMode("file"):
        args = cmd_args([
            tools.file_cycles,
            "--target-data",
            target_data,
            "--module-name",
            module_name,
            "--target-path",
            str(ctx.label.raw_target()),
            "--output",
            out.as_output(),
        ])
        if swift_dot != None:
            # Swift compilation deliberately uses configuration-based paths.
            # Copy the finished graph into the content-addressed namespace so
            # this analysis action can still dedupe when two configurations
            # produced byte-for-byte identical dependency graphs. The cheap
            # copy itself remains configuration-specific.
            content_based_swift_dot = _content_based_input(ctx, swift_dot, "swift-dependencies.dot")
            args.add("--swift-dot", content_based_swift_dot)
    else:
        args = cmd_args([
            tools.package_cycles,
            "--target-data",
            target_data,
            "--output",
            out.as_output(),
        ])
    ctx.actions.run(
        args,
        category = "target_stats_cycles",
        allow_cache_upload = True,
        expect_eligible_for_dedupe = True,
    )
    return out

def _all_target_stats_subtarget(ctx: AnalysisContext, tset: TargetStatsInfoTSet) -> list[Provider]:
    """A JSON object mapping every transitive target's label to its manifest."""
    manifest_by_target = {record.label: record.manifest for record in tset.traverse()}
    out = ctx.actions.declare_output(_OUT_DIR, "all_target_stats.json", has_content_based_path = True)
    out_with_inputs = ctx.actions.write_json(out, manifest_by_target, with_inputs = True)
    return [
        DefaultInfo(
            default_output = out,
            other_outputs = [out_with_inputs, tset.project_as_args("manifests")],
        )
    ]

def target_stats_aggregate_providers_and_subtargets(
    ctx: AnalysisContext,
    *,
    deps: list[Dependency],
) -> (list[Provider], dict[str, list[Provider]]):
    """target_stats for a rule that contributes no sources of its own.

    A no-op when target_stats is disabled, so aggregating rules can call it
    unconditionally.
    """
    if not TARGET_STATS_ENABLED:
        return [], {}
    children = [dep[TargetStatsInfo].tset for dep in deps if dep.get(TargetStatsInfo) != None]
    tset = ctx.actions.tset(TargetStatsInfoTSet, children = children)
    info = TargetStatsInfo(label = str(ctx.label.raw_target()), tset = tset)
    subtargets = {
        "all_target_stats": _all_target_stats_subtarget(ctx, tset),
    }
    return [info], subtargets

def target_stats_providers_and_subtargets(
    ctx: AnalysisContext,
    *,
    tools: TargetStatsToolsInfo,
    srcs: dict[str, Artifact],
    deps: list[Dependency],
    cycle_mode: CycleMode,
    module_name: str,
    swift_dot: Artifact | None = None,
) -> (list[Provider], dict[str, list[Provider]]):
    """Build the target_stats actions, provider and sub-targets for one target."""
    label = str(ctx.label.raw_target())

    file_stats_by_path = {}
    for name, src in srcs.items():
        # A header map key need not be normalized, and it becomes an output
        # path. third-party/libvpx maps each generated header under both "name"
        # and "./name", so the same file arrives twice and normalizes to one
        # name; measure it once rather than declaring the output twice.
        name = paths.normalize(name)
        if name in file_stats_by_path:
            continue
        file_stats_by_path[name] = _file_stats_action(ctx, tools, src, name)

    target_data = _extract_target_data_action(ctx, tools, file_stats_by_path)
    cycles = _cycles_action(ctx, tools, cycle_mode, target_data, module_name, swift_dot)

    manifest = ctx.actions.declare_output(_OUT_DIR, "target_stats.json", has_content_based_path = True)
    manifest_inputs = ctx.actions.write_json(
        manifest,
        {
            "cycles": cycles,
            "file_stats": file_stats_by_path,
            "target": label,
            "target_data": target_data,
        },
        with_inputs = True,
    )

    children = [dep[TargetStatsInfo].tset for dep in deps if dep.get(TargetStatsInfo) != None]
    tset = ctx.actions.tset(
        TargetStatsInfoTSet,
        value = TargetStatsRecord(
            label = label,
            manifest = manifest,
            manifest_with_inputs = manifest_inputs,
        ),
        children = children,
    )
    info = TargetStatsInfo(label = label, tset = tset)

    subtargets = {
        "all_target_stats": _all_target_stats_subtarget(ctx, tset),
        "target_stats": [DefaultInfo(default_output = manifest, other_outputs = [manifest_inputs]), info],
    }
    return [info], subtargets
