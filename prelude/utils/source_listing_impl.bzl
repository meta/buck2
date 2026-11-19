# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

SourceListingInfo = provider(
    fields = {
        "sources": dict[str, Artifact],
    }
)

SourceListingInfoAlias = SourceListingInfo

def _impl(ctx):
    sources = {}
    for d in ctx.attrs.deps:
        package = ctx.label.package
        if package != "":
            package += "/"
        rel_loc = d.label.package.removeprefix(package)
        sources.update({rel_loc + "/" + p: art for p, art in d[SourceListingInfo].sources.items()})

    for s in ctx.attrs.srcs:
        sources[s.short_path] = s
    return [DefaultInfo(), SourceListingInfo(sources = sources)]

# This rule acts sort of like a `filegroup`, except that 1) it returns all the
# source artifacts unchanged, and 2) it reports the location of all artifacts
# relative to the current package. We use this for gathering listings of the
# source files for bundled cells.
_source_listing = rule(
    impl = _impl,
    attrs = {
        "deps": attrs.list(attrs.dep()),
        "srcs": attrs.list(attrs.source()),
    },
)

def source_listing_impl(exclude: list[str]):
    package = package_name()
    if package != "":
        package += "/"
    _source_listing(
        name = "source_listing",
        srcs = glob(["**/*", "**/.*"], exclude = exclude),
        deps = ["//" + package + s + ":source_listing" for s in __internal__.sub_packages()],
        visibility = ["PUBLIC"],
    )

def empty_source_listing_impl():
    _source_listing(
        name = "source_listing",
        srcs = [],
        deps = [],
        visibility = ["PUBLIC"],
    )
