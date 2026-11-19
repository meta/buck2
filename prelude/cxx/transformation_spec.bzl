# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load(
    "@prelude//utils:build_graph_pattern.bzl",
    "BuildGraphInfo",
    "BuildGraphPattern",
    "parse_build_graph_pattern",
)
load(
    "@prelude//utils:build_target_pattern.bzl",
    "BuildTargetPattern",
    "parse_build_target_pattern",
)
load(
    "@prelude//utils:strings.bzl",
    "strip_prefix",
)

TransformationKind = enum(
    "debug",
    "optimized",
)

TransformationResultProvider = provider(
    fields = {
        "determine_transformation": provider_field(typing.Callable[[Label, BuildGraphInfo], TransformationKind | None]),
        "is_empty": provider_field(bool),
    }
)

TransformationTest = record(
    matcher = Label | BuildTargetPattern | BuildGraphPattern,
    result = TransformationKind,
)

TransformationSpecContext = record(
    provider = TransformationResultProvider,
    graph_info = BuildGraphInfo,
)

def build_transformation_spec_context(ctx: AnalysisContext, graph_info: BuildGraphInfo) -> TransformationSpecContext | None:
    transformation_provider = (
        ctx.attrs.transformation_spec[TransformationResultProvider] if (hasattr(ctx.attrs, "transformation_spec") and ctx.attrs.transformation_spec) else None
    )
    if not transformation_provider:
        return None

    return TransformationSpecContext(
        provider = transformation_provider,
        graph_info = graph_info,
    )

def _build_pattern_matcher(entry: str) -> BuildTargetPattern | None:
    pattern = strip_prefix("pattern:", entry)
    if pattern == None:
        return None
    return parse_build_target_pattern(pattern)

def _build_graph_matcher(entry: str) -> BuildGraphPattern | None:
    pattern = strip_prefix("graph:", entry)
    if pattern == None:
        return None
    return parse_build_graph_pattern(pattern)

def _build_transformations(transformations: list[(str | Dependency, str)]) -> list[TransformationTest]:
    parsed_transformations = []
    for entry in transformations:
        kind = TransformationKind(entry[1])
        if isinstance(entry[0], Dependency):
            parsed_transformations.append(
                TransformationTest(
                    matcher = entry[0].label,
                    result = kind,
                )
            )
            continue

        target_pattern_matcher = _build_pattern_matcher(entry[0])
        if target_pattern_matcher:
            parsed_transformations.append(
                TransformationTest(
                    matcher = target_pattern_matcher,
                    result = kind,
                )
            )
            continue

        graph_matcher = _build_graph_matcher(entry[0])
        if graph_matcher:
            parsed_transformations.append(
                TransformationTest(
                    matcher = graph_matcher,
                    result = kind,
                )
            )
            continue

        fail("Invalid matcher provided: " + entry[0])
    return parsed_transformations

def build_determine_transformation(transformations: list[TransformationTest]) -> typing.Callable[[Label, BuildGraphInfo], TransformationKind | None]:
    def callable(label: Label, graph_info: BuildGraphInfo) -> TransformationKind | None:
        for transform in transformations:
            if isinstance(transform.matcher, BuildTargetPattern) and transform.matcher.matches(label):
                return transform.result
            elif isinstance(transform.matcher, Label) and transform.matcher == label:
                return transform.result
            elif isinstance(transform.matcher, BuildGraphPattern) and transform.matcher.matches(label, graph_info):
                return transform.result

        return None

    return callable

def transformation_spec_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        TransformationResultProvider(
            is_empty = len(ctx.attrs.transformations) == 0,
            determine_transformation = build_determine_transformation(_build_transformations(ctx.attrs.transformations)),
        ),
    ]

transformation_spec = prelude_rule(
    name = "transformation_spec",
    impl = transformation_spec_impl,
    docs = """
        A `transformation_spec()` rule declares a list of transformation
        rules that map targets (by `Label`, by build target pattern, or
        by build graph pattern) to a `TransformationKind`
        (`debug` or `optimized`). Other rules can reference a
        `transformation_spec()` via a `transformation_spec` attribute to
        decide, on a per-dependency basis, which transformation should be
        applied when building that dependency.
        Exact `debug` and `optimized` meaning depends on
        flags configured in toolchain for each flavor.
    """,
    examples = None,
    further = None,
    attrs = {
        "transformations": attrs.list(
            attrs.tuple(
                attrs.one_of(attrs.dep(), attrs.string()),
                attrs.enum(TransformationKind.values()),
            ),
            default = [],
        ),
    },
)
