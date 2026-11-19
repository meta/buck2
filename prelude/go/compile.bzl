# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(
    ":packages.bzl",
    "GoPkg",  # @Unused used as type
    "merge_pkgs",
)

# Provider wrapping packages used for compiling.
GoPkgCompileInfo = provider(
    fields = {
        "pkgs": provider_field(typing.Any, default = None),  # dict[str, GoPkg]
    }
)

# Provider for test targets that test a library. Contains information for
# compiling the test and library code together as expected by go.
GoTestInfo = provider(
    # @unsorted-dict-items
    fields = {
        "deps": provider_field(typing.Any, default = None),  # [Dependency]
        "srcs": provider_field(typing.Any, default = None),  # ["source"]
        "pkg_import_path": provider_field(typing.Any, default = None),  # str
        "coverage_enabled": provider_field(bool),
    },
)

def get_inherited_compile_pkgs(deps: list[Dependency]) -> dict[str, GoPkg]:
    return merge_pkgs([d[GoPkgCompileInfo].pkgs for d in deps if GoPkgCompileInfo in d])

def infer_package_root(srcs: list[Artifact]) -> str:
    go_sources = [s for s in srcs if s.extension == ".go"]
    if len(go_sources) == 0:
        return ""
    dir_set = set([paths.dirname(s.short_path) for s in go_sources])
    if len(dir_set) > 1:
        fail(
            "Provide `package_root` target attribute. Can't infer it when there are multiple directories containing .go files: {}. Sources: {}".format(
                dir_set,
                [s.short_path for s in go_sources],
            )
        )

    return list(dir_set)[0]
