# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(":package_root.bzl", "strip_package_root")
load(":toolchain.bzl", "GoToolchainInfo", "get_toolchain_env_vars")

GoListError = record(
    err = field(str),
)

# Modeled after: https://pkg.go.dev/cmd/go/internal/list#pkg-variables
GoListOut = record(
    name = field(str),
    imports = field(list[str], default = []),
    test_imports = field(list[str], default = []),
    go_files = field(list[Artifact], default = []),
    h_files = field(list[Artifact], default = []),
    c_files = field(list[Artifact], default = []),
    cxx_files = field(list[Artifact], default = []),
    cgo_files = field(list[Artifact], default = []),
    s_files = field(list[Artifact], default = []),
    syso_files = field(list[Artifact], default = []),
    test_go_files = field(list[Artifact], default = []),
    x_test_go_files = field(list[Artifact], default = []),
    ignored_go_files = field(list[Artifact], default = []),
    ignored_other_files = field(list[Artifact], default = []),
    cgo_cflags = field(list[str], default = []),
    cgo_cppflags = field(list[str], default = []),
    embed_patterns = field(list[str], default = []),
    test_embed_patterns = field(list[str], default = []),
    error = field(GoListError | None, default = None),
)

GoListDynamicValue = provider(
    fields = {
        "go_list_out": provider_field(GoListOut),
    },
)

def go_list(
    actions: AnalysisActions,
    go_toolchain: GoToolchainInfo,
    pkg_import_path: str,
    srcs: list[Artifact],
    package_root: str,
    build_tags: list[str],
    cgo_enabled: bool,
    with_tests: bool,
) -> DynamicValue:
    env = get_toolchain_env_vars(go_toolchain)

    go_list_out = actions.declare_output(paths.basename(pkg_import_path) + "_go_list.json", has_content_based_path = True)

    srcs_dir = actions.symlinked_dir(
        "__{}_srcs_dir__".format(paths.basename(pkg_import_path)),
        {strip_package_root(src.short_path, package_root): src for src in srcs},
        has_content_based_path = True,
    )
    all_tags = [] + go_toolchain.build_tags + build_tags

    go_list_args = [
        go_toolchain.pkg_analyzer,
        ["-o", go_list_out.as_output()],
        ["-goos", go_toolchain.env_go_os],
        ["-goarch", go_toolchain.env_go_arch],
        [["-tags", ",".join(all_tags)] if all_tags else []],
        ["-race"] if go_toolchain.race and cgo_enabled else [],
        ["-asan"] if go_toolchain.asan and cgo_enabled else [],
        ["-cgo"] if cgo_enabled else [],
        ["-tests"] if with_tests else [],
        srcs_dir,
    ]

    identifier = paths.basename(pkg_import_path)
    actions.run(go_list_args, env = env, category = "go_list", identifier = identifier, allow_cache_upload = go_toolchain.allow_cache_upload)

    return actions.dynamic_output_new(
        _parse_go_list(
            srcs = srcs,
            package_root = package_root,
            go_list_out = go_list_out,
        )
    )

# buildifier: disable=unused-variable
def _parse_go_list_impl(actions: AnalysisActions, srcs: list[Artifact], package_root: str, go_list_out: ArtifactValue) -> list[Provider]:
    _ = actions
    go_list = go_list_out.read_json()
    go_files, cgo_files, h_files, c_files, cxx_files, s_files, syso_files, test_go_files, x_test_go_files, ignored_go_files, ignored_other_files = (
        [],
        [],
        [],
        [],
        [],
        [],
        [],
        [],
        [],
        [],
        [],
    )

    for src in srcs:
        # remove package_root prefix from src artifact path to match `go list` output format
        src_path = strip_package_root(src.short_path, package_root)
        if src_path in go_list.get("GoFiles", []):
            go_files.append(src)
        if src_path in go_list.get("CgoFiles", []):
            cgo_files.append(src)
        if src_path in go_list.get("HFiles", []):
            h_files.append(src)
        if src_path in go_list.get("CFiles", []):
            c_files.append(src)
        if src_path in go_list.get("CXXFiles", []):
            cxx_files.append(src)
        if src_path in go_list.get("SFiles", []):
            s_files.append(src)
        if src_path in go_list.get("SysoFiles", []):
            syso_files.append(src)
        if src_path in go_list.get("TestGoFiles", []):
            test_go_files.append(src)
        if src_path in go_list.get("XTestGoFiles", []):
            x_test_go_files.append(src)
        if src_path in go_list.get("IgnoredGoFiles", []):
            ignored_go_files.append(src)
        if src_path in go_list.get("IgnoredOtherFiles", []):
            ignored_other_files.append(src)

    name = go_list.get("Name", "")
    imports = go_list.get("Imports", [])
    test_imports = go_list.get("TestImports", [])
    cgo_cflags = go_list.get("CgoCFLAGS", [])
    cgo_cppflags = go_list.get("CgoCPPFLAGS", [])
    embed_patterns = go_list.get("EmbedPatterns", [])
    test_embed_patterns = go_list.get("TestEmbedPatterns", [])
    error = _parse_error(go_list.get("Error", None))

    go_list = GoListOut(
        name = name,
        imports = imports,
        test_imports = test_imports,
        go_files = go_files,
        h_files = h_files,
        c_files = c_files,
        cxx_files = cxx_files,
        cgo_files = cgo_files,
        s_files = s_files,
        syso_files = syso_files,
        test_go_files = test_go_files,
        x_test_go_files = x_test_go_files,
        cgo_cflags = cgo_cflags,
        cgo_cppflags = cgo_cppflags,
        ignored_go_files = ignored_go_files,
        ignored_other_files = ignored_other_files,
        embed_patterns = embed_patterns,
        test_embed_patterns = test_embed_patterns,
        error = error,
    )
    return [GoListDynamicValue(go_list_out = go_list)]

_parse_go_list = dynamic_actions(
    impl = _parse_go_list_impl,
    attrs = {
        "go_list_out": dynattrs.artifact_value(),
        "package_root": dynattrs.value(str),
        "srcs": dynattrs.value(list[Artifact]),
    },
)

def _parse_error(error: dict | None) -> GoListError | None:
    if error == None:
        return None

    if "Err" not in error:
        # Should never happen, but if it does, fail the build.
        fail("Invalid go_analyzer error: {}".format(error))

    return GoListError(err = error.get("Err"))
