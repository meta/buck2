# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

ClangProfileListInfo = provider(
    fields = {
        "profile_list": provider_field(Artifact),
        "srcs": provider_field(list[str]),
    }
)

def get_coverage_profile_list_artifact(dep: [Dependency, None]) -> [Artifact, None]:
    """Extract the profile list artifact from a clang_profile_list dependency."""
    if dep == None:
        return None
    info = dep.get(ClangProfileListInfo)
    if info == None:
        return None
    return info.profile_list

def get_coverage_profile_list_srcs(dep: Dependency | None) -> list[str] | None:
    if dep == None:
        return None
    info = dep.get(ClangProfileListInfo)
    return info.srcs if info else None

def _clang_profile_list_impl(ctx: AnalysisContext) -> list[Provider]:
    """
    Generates a clang profile list file for use with -fprofile-list.

    When `srcs` is non-empty (selective coverage), generates a file that
    instruments only the listed source files:
        [clang]
        source:<path>=allow
        ...
        default:skip

    When `srcs` is empty (whole coverage), generates a file that instruments
    everything:
        [clang]
        default:allow
    """
    lines = ["[clang]"]
    for src_path in ctx.attrs.srcs:
        # Use wildcard prefix to match both direct source paths (fbcode/path/to/file.cpp)
        # and symlink tree paths (buck-out/.../buck-headers/path/to/file.h)
        lines.append("source:*{}=allow".format(src_path))
    if ctx.attrs.srcs:
        lines.append("default:skip")
    else:
        lines.append("default:allow")
    lines.append("")

    profile_list = ctx.actions.write("profile_list.txt", "\n".join(lines), has_content_based_path = True)

    return [
        DefaultInfo(default_output = profile_list),
        ClangProfileListInfo(profile_list = profile_list, srcs = ctx.attrs.srcs),
    ]

clang_profile_list = rule(
    impl = _clang_profile_list_impl,
    attrs = {
        "srcs": attrs.list(attrs.string(), default = []),
    },
)
