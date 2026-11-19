# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _artifacts(value: Artifact):
    return value

TransitiveOutputsTSet = transitive_set(args_projections = {"artifacts": _artifacts})

JsLibraryInfo = provider(
    fields = {
        # Output of the `library_files` action: the library's own transformed
        # files, before cross-library dependency resolution.
        "library_files": provider_field(Artifact | None, default = None),
        "output": provider_field(Artifact | None, default = None),
        "transitive_outputs": provider_field(TransitiveOutputsTSet | None, default = None),
    },
)

JsBundleInfo = provider(
    # @unsorted-dict-items
    fields = {
        "bundle_name": provider_field(typing.Any, default = None),  # str
        # Directory containing the built JavaScript.
        "built_js": provider_field(typing.Any, default = None),  # "artifact",
        # Source map belonging to the built JavaScript.
        "source_map": provider_field(typing.Any, default = None),  # "artifact",
        # Directory containing the resources (or assets) used by the bundled JavaScript source code.
        "res": provider_field(typing.Any, default = None),  # ["artifact", None]
        # Directory of raw asset files produced by js_library targets that use
        # `_asset_dest_path_resolver = "generic"`. Only used on Android platforms.
        "generic_assets": provider_field(typing.Any, default = None),  # ["artifact", None]
        # Directory containing various metadata that can be used by dependent rules but are not
        # meant to be shipped with the application.
        "misc": provider_field(typing.Any, default = None),  # "artifact"
        # Dependencies graph file associated with the built JavaScript.
        "dependencies_file": provider_field(typing.Any, default = None),  # "artifact"
    },
)

def get_transitive_outputs(actions: AnalysisActions, value: Artifact | None = None, deps: list[JsLibraryInfo] = []) -> TransitiveOutputsTSet:
    kwargs = {}
    if value:
        kwargs["value"] = value
    if deps:
        kwargs["children"] = [js_library_info.transitive_outputs for js_library_info in deps]

    return actions.tset(TransitiveOutputsTSet, **kwargs)
