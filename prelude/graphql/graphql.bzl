# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",
    "make_artifact_tset",
)
load(
    "@prelude//cxx:cxx_library_utility.bzl",
    "cxx_attr_deps",
    "cxx_attr_exported_deps",
)

GraphQLiOSInfo = record(
    header_path_prefix = str | None,
    force_export_fragment_headers = bool,
    fragment_models_use_classes = bool,
    use_implicit_codegen = bool,
)

GraphQLAndroidInfo = record(
    allow_legacy_fragments = bool,
    allow_type_models = bool,
    composition_mode = str | None,
    enable_associated_library = bool,
    use_implicit_codegen = bool,
)

GraphQLCxxInfo = record(
    use_implicit_codegen = bool,
)

GraphQLInfo = provider(
    fields = {
        "config_name": provider_field(str),
        "enforce_colocation": provider_field(bool),
        "is_subconfig": provider_field(bool),
        "is_test_target": provider_field(bool),
        "platform_config": provider_field(GraphQLAndroidInfo | GraphQLCxxInfo | GraphQLiOSInfo),
        "srcs": provider_field(list[Artifact]),
        "target": provider_field(Label),
    },
)

GraphQLCodegenInput = provider(
    fields = {
        "artifacts": provider_field(ArtifactTSet),
    },
)

GraphQLMobileConfigInput = provider(
    fields = {
        "artifacts": provider_field(ArtifactTSet),
    },
)

GraphQLMockDataInput = provider(
    fields = {
        "artifacts": provider_field(ArtifactTSet),
    },
)

def _graphql_info_provider(label: Label, deps: list[Dependency]) -> list[Provider]:
    return [dep[GraphQLInfo] for dep in deps if GraphQLInfo in dep and dep[GraphQLInfo].target == label]

def graphql_codegen_sets(deps: list[Dependency]) -> list[ArtifactTSet]:
    return [dep[GraphQLCodegenInput].artifacts for dep in deps if GraphQLCodegenInput in dep]

def graphql_mobileconfig_sets(deps: list[Dependency]) -> list[ArtifactTSet]:
    return [dep[GraphQLMobileConfigInput].artifacts for dep in deps if GraphQLMobileConfigInput in dep]

def graphql_mock_data_sets(deps: list[Dependency]) -> list[ArtifactTSet]:
    return [dep[GraphQLMockDataInput].artifacts for dep in deps if GraphQLMockDataInput in dep]

def graphql_providers(ctx: AnalysisContext) -> list[Provider]:
    deps = cxx_attr_deps(ctx)
    providers = _graphql_info_provider(ctx.label, deps)
    if len(providers) > 1:
        fail("Multiple GraphQLInfo providers found in deps of {}".format(ctx.label.raw_target()))

    all_deps = deps + cxx_attr_exported_deps(ctx) + getattr(ctx.attrs, "runtime_deps", [])
    output_sets = graphql_codegen_sets(all_deps)
    if output_sets:
        providers.append(
            GraphQLCodegenInput(
                artifacts = make_artifact_tset(actions = ctx.actions, children = output_sets),
            ),
        )

    mobileconfig_sets = graphql_mobileconfig_sets(all_deps)
    if mobileconfig_sets:
        providers.append(
            GraphQLMobileConfigInput(
                artifacts = make_artifact_tset(actions = ctx.actions, children = mobileconfig_sets),
            ),
        )

    mock_data_sets = graphql_mock_data_sets(all_deps)
    if mock_data_sets:
        providers.append(
            GraphQLMockDataInput(
                artifacts = make_artifact_tset(actions = ctx.actions, children = mock_data_sets),
            ),
        )

    return providers
