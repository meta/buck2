# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Extra relinker outputs support (e.g., LLVM stats).

Reads from ctx.attrs.extra_relinker_outputs attribute (not buck config directly).
Buck config is read at macro level and translated to attribute value.
"""

load("@prelude//linking:link_info.bzl", "ExtraLinkerOutputs")
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused Used as a type

def _llvm_stats_outputs_factory(output_path: str) -> typing.Callable:
    """Factory for relinker LLVM stats outputs."""

    def factory(ctx: AnalysisContext) -> ExtraLinkerOutputs:
        stats_file = ctx.actions.declare_output(output_path + ".stats", has_content_based_path = False)
        return ExtraLinkerOutputs(
            artifacts = {"llvm-stats": stats_file},
            providers = {"llvm-stats": [DefaultInfo(default_output = stats_file)]},
        )

    return factory

def _llvm_stats_flags_factory() -> typing.Callable:
    """Factory for relinker LLVM stats linker flags."""

    def factory(_ctx: AnalysisContext, outputs: dict[str, Artifact]) -> list[ArgLike]:
        return [
            cmd_args(outputs["llvm-stats"].as_output(), format = "-Wl,-plugin-opt=stats-file={}"),
        ]

    return factory

def _preinline_ir_outputs_factory(output_path: str) -> typing.Callable:
    """Factory for the relinker pre-inline IR (.ll) output."""

    def factory(ctx: AnalysisContext) -> ExtraLinkerOutputs:
        ir_file = ctx.actions.declare_output(output_path + ".preinline.ll", has_content_based_path = False)
        return ExtraLinkerOutputs(
            artifacts = {"preinline-ir": ir_file},
            providers = {"preinline-ir": [DefaultInfo(default_output = ir_file)]},
        )

    return factory

def _preinline_ir_flags_factory() -> typing.Callable:
    """Factory for the relinker pre-inline IR linker flags.

    Threads the gator `-full-lto-postlink-preinline-ir-dump=<file>` mllvm option
    into the LTO backend. `.as_output()` is what registers the linker-written
    file as a tracked, materializable buck output (a bare `-Wl,-mllvm` string
    would be discarded by RE)."""

    def factory(_ctx: AnalysisContext, outputs: dict[str, Artifact]) -> list[ArgLike]:
        return [
            cmd_args(
                outputs["preinline-ir"].as_output(),
                format = "-Wl,-mllvm,-full-lto-postlink-preinline-ir-dump={}",
            ),
        ]

    return factory

def _cir_opt_report_outputs_factory(output_path: str) -> typing.Callable:
    """Factory for the relinker ClangIR optimization report outputs."""

    def factory(ctx: AnalysisContext) -> ExtraLinkerOutputs:
        stats_file = ctx.actions.declare_output(output_path + ".cir-stats", has_content_based_path = False)
        remarks_file = ctx.actions.declare_output(output_path + ".cir-remarks.jsonl", has_content_based_path = False)
        return ExtraLinkerOutputs(
            artifacts = {"cir-opt-remarks": remarks_file, "cir-opt-stats": stats_file},
            # Keyed by the output type, which is what the [relinker_extra_outputs]
            # sub-target looks up.
            providers = {"cir-opt-report": [DefaultInfo(default_outputs = [stats_file, remarks_file])]},
        )

    return factory

def _cir_opt_report_flags_factory() -> typing.Callable:
    """Factory for the relinker ClangIR optimization report linker flags.

    lld reads the .cir_opportunity sections of the library's objects, the LTO
    output included, and writes the summed counters and the remark sites."""

    def factory(_ctx: AnalysisContext, outputs: dict[str, Artifact]) -> list[ArgLike]:
        return [
            cmd_args(outputs["cir-opt-stats"].as_output(), format = "-Wl,--cir-opt-stats={}"),
            cmd_args(outputs["cir-opt-remarks"].as_output(), format = "-Wl,--cir-opt-remarks={}"),
        ]

    return factory

def _get_extra_relinker_output_types(ctx: AnalysisContext) -> list[str]:
    """Get list of enabled extra relinker output types from attribute."""
    return getattr(ctx.attrs, "extra_relinker_outputs", [])

def get_extra_relinker_args(ctx: AnalysisContext, output_path: str, soname: str = "") -> dict[str, typing.Any]:
    """Get extra linker outputs factory dict for all enabled relinker outputs.

    `soname` scopes the (multi-GB) pre-inline IR dump: when `preinline_ir_sonames`
    is set, only libraries in that allowlist emit the dump output+flag, so a whole
    -app build does not dump every library. An empty allowlist keeps legacy
    behavior (every relinked library dumps)."""
    output_types = _get_extra_relinker_output_types(ctx)
    allowlist = getattr(ctx.attrs, "preinline_ir_sonames", [])
    if allowlist and "preinline-ir" in output_types and soname not in allowlist:
        output_types = [t for t in output_types if t != "preinline-ir"]
    if not output_types:
        return {}

    def combined_outputs_factory(ctx: AnalysisContext) -> ExtraLinkerOutputs:
        artifacts = {}
        providers = {}
        for output_type in output_types:
            if output_type == "llvm-stats":
                result = _llvm_stats_outputs_factory(output_path)(ctx)
                artifacts |= result.artifacts
                providers |= result.providers
            elif output_type == "preinline-ir":
                result = _preinline_ir_outputs_factory(output_path)(ctx)
                artifacts |= result.artifacts
                providers |= result.providers
            elif output_type == "cir-opt-report":
                result = _cir_opt_report_outputs_factory(output_path)(ctx)
                artifacts |= result.artifacts
                providers |= result.providers
        return ExtraLinkerOutputs(artifacts = artifacts, providers = providers)

    def combined_flags_factory(ctx: AnalysisContext, outputs: dict[str, Artifact]) -> list[ArgLike]:
        flags = []
        for output_type in output_types:
            if output_type == "llvm-stats":
                flags += _llvm_stats_flags_factory()(ctx, outputs)
            elif output_type == "preinline-ir":
                flags += _preinline_ir_flags_factory()(ctx, outputs)
            elif output_type == "cir-opt-report":
                flags += _cir_opt_report_flags_factory()(ctx, outputs)
        return flags

    return {
        "extra_linker_outputs_factory": combined_outputs_factory,
        "extra_linker_outputs_flags_factory": combined_flags_factory,
    }
