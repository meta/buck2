# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def needs_instrumentation(children: list[bool], contains_headers_selected_for_coverage_instrumentation: bool) -> bool:
    return contains_headers_selected_for_coverage_instrumentation or any(children)

CxxExportedNeedsCoverageInstrumentationTSet = transitive_set(
    reductions = {
        "needs_instrumentation": needs_instrumentation,
    },
)

CxxExportedNeedsCoverageInstrumentation = provider(
    fields = {
        "nodes": CxxExportedNeedsCoverageInstrumentationTSet,
    }
)

def build_needs_coverage_tset(ctx: AnalysisContext, deps: list[Dependency]) -> CxxExportedNeedsCoverageInstrumentationTSet:
    return ctx.actions.tset(
        CxxExportedNeedsCoverageInstrumentationTSet,
        value = ctx.attrs.exported_needs_coverage_instrumentation if hasattr(ctx.attrs, "exported_needs_coverage_instrumentation") else False,
        children = [d.get(CxxExportedNeedsCoverageInstrumentation).nodes for d in deps if d.get(CxxExportedNeedsCoverageInstrumentation) != None],
    )

def build_exported_needs_coverage(ctx: AnalysisContext, deps: list[Dependency]) -> CxxExportedNeedsCoverageInstrumentation:
    return CxxExportedNeedsCoverageInstrumentation(
        nodes = build_needs_coverage_tset(ctx, deps),
    )

def is_coverage_enabled_by_any_dep(ctx: AnalysisContext, deps: list[Dependency]) -> bool:
    tset = build_needs_coverage_tset(ctx, deps)

    return tset.reduce("needs_instrumentation")

def needs_coverage(cxx_exported_needs_coverage: CxxExportedNeedsCoverageInstrumentation) -> bool:
    return cxx_exported_needs_coverage.nodes.reduce("needs_instrumentation")
