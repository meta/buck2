# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""The Swift sources an app was built from, for hot reload.

A library compiled for Swift dynamic replacement contributes its Swift sources
to a transitive set; binaries and bundles below the app forward it; the app
bundle (`apple_bundle.bzl`) packs every source with its owning target into
`SwiftSources.json.zst` next to `BuildInfo.json`. A hot reload session reads that file instead of
asking source control for the built text and Buck for the owner, so the
session does not depend on the checkout being at the build's commit.
"""

SWIFT_SOURCES_FILE_NAME = "SwiftSources.json.zst"

SwiftDynamicReplacementSources = record(
    label = field(Label),
    sources = field(list[Artifact]),
)

def _listing(value: SwiftDynamicReplacementSources) -> cmd_args:
    # One `target<TAB>path` line per source; the path is the source artifact's
    # project-relative path, which is the one the session's edits name.
    return cmd_args([cmd_args(str(value.label.raw_target()), source, delimiter = "\t") for source in value.sources])

SwiftDynamicReplacementSourcesTSet = transitive_set(
    args_projections = {
        "listing": _listing,
    },
)

SwiftDynamicReplacementSourcesInfo = provider(
    fields = {
        "sources": provider_field(SwiftDynamicReplacementSourcesTSet),
    },
)

def swift_dynamic_replacement_sources_info(
    ctx: AnalysisContext, deps: list[Dependency], own_sources: list[Artifact]
) -> [SwiftDynamicReplacementSourcesInfo, None]:
    """The provider for a target that compiles `own_sources` for dynamic
    replacement (empty when it does not) and depends on `deps`; None when
    neither it nor anything below it has such sources."""
    children = [dep[SwiftDynamicReplacementSourcesInfo].sources for dep in deps if SwiftDynamicReplacementSourcesInfo in dep]
    if not own_sources and not children:
        return None
    if not own_sources and len(children) == 1:
        return SwiftDynamicReplacementSourcesInfo(sources = children[0])
    kwargs = {}
    if own_sources:
        kwargs["value"] = SwiftDynamicReplacementSources(label = ctx.label, sources = own_sources)
    if children:
        kwargs["children"] = children
    return SwiftDynamicReplacementSourcesInfo(sources = ctx.actions.tset(SwiftDynamicReplacementSourcesTSet, **kwargs))
