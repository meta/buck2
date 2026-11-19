# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//haskell:library_info.bzl",
    "HaskellLibraryInfo",
    "HaskellLibraryProvider",
)
load(
    "@prelude//haskell:link_info.bzl",
    "HaskellLinkInfo",
    "HaskellProfLinkInfo",
)
load(
    "@prelude//linking:link_info.bzl",
    "LinkStyle",
    "MergedLinkInfo",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
)

HASKELL_EXTENSIONS = [
    ".hs",
    ".lhs",
    ".hsc",
    ".chs",
    ".x",
    ".y",
]

# We take a named_set for srcs, which is sometimes a list, sometimes a dict.
# In future we should only accept a list, but for now, cope with both.
def srcs_to_pairs(srcs) -> list[(str, Artifact)]:
    if type(srcs) == type({}):
        return srcs.items()
    else:
        return [(src.short_path, src) for src in srcs]

def is_haskell_src(x: str) -> bool:
    _, ext = paths.split_extension(x)
    return ext in HASKELL_EXTENSIONS

def src_to_module_name(x: str) -> str:
    base, _ext = paths.split_extension(x)
    return base.replace("/", ".")

def attr_deps(ctx: AnalysisContext) -> list[Dependency]:
    return ctx.attrs.deps

def attr_deps_haskell_link_infos(ctx: AnalysisContext) -> list[HaskellLinkInfo]:
    return dedupe(
        filter(
            None,
            [d.get(HaskellLinkInfo) for d in attr_deps(ctx) + ctx.attrs.template_deps],
        )
    )

# DONT CALL THIS FUNCTION, you want attr_deps_haskell_link_infos instead
def attr_deps_haskell_link_infos_sans_template_deps(ctx: AnalysisContext) -> list[HaskellLinkInfo]:
    return dedupe(
        filter(
            None,
            [d.get(HaskellLinkInfo) for d in attr_deps(ctx)],
        )
    )

def attr_deps_haskell_lib_infos(ctx: AnalysisContext, link_style: LinkStyle, enable_profiling: bool) -> list[HaskellLibraryInfo]:
    if enable_profiling and link_style == LinkStyle("shared"):
        fail("Profiling isn't supported when using dynamic linking")
    return [
        x.prof_lib[link_style] if enable_profiling else x.lib[link_style]
        for x in filter(None, [d.get(HaskellLibraryProvider) for d in attr_deps(ctx) + ctx.attrs.template_deps])
    ]

def attr_deps_merged_link_infos(ctx: AnalysisContext) -> list[MergedLinkInfo]:
    return dedupe(
        filter(
            None,
            [d.get(MergedLinkInfo) for d in attr_deps(ctx)],
        )
    )

def attr_deps_profiling_link_infos(ctx: AnalysisContext) -> list[MergedLinkInfo]:
    return filter(
        None,
        [d.get(HaskellProfLinkInfo).prof_infos if d.get(HaskellProfLinkInfo) else d.get(MergedLinkInfo) for d in attr_deps(ctx)],
    )

def attr_deps_shared_library_infos(ctx: AnalysisContext) -> list[SharedLibraryInfo]:
    return filter(
        None,
        [d.get(SharedLibraryInfo) for d in attr_deps(ctx)],
    )

def _link_style_extensions(link_style: LinkStyle) -> (str, str):
    if link_style == LinkStyle("shared"):
        return ("dyn_o", "dyn_hi")
    elif link_style == LinkStyle("static_pic"):
        return ("o", "hi")  # is this right?
    elif link_style == LinkStyle("static"):
        return ("o", "hi")
    fail("unknown LinkStyle")

def output_extensions(link_style: LinkStyle, profiled: bool) -> (str, str):
    osuf, hisuf = _link_style_extensions(link_style)
    if profiled:
        return ("p_" + osuf, "p_" + hisuf)
    else:
        return (osuf, hisuf)

# Single place to build the suffix used in artifacts (e.g. package directories,
# lib names) considering attributes like link style and profiling.
def get_artifact_suffix(link_style: LinkStyle, enable_profiling: bool, suffix: str = "") -> str:
    artifact_suffix = link_style.value
    if enable_profiling:
        artifact_suffix += "-prof"
    return artifact_suffix + suffix
