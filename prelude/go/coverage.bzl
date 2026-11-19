# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":toolchain.bzl", "GoToolchainInfo", "get_toolchain_env_vars")

GoCoverageMode = enum(
    "set",
    "count",
    "atomic",
)

def cover_srcs(
    actions: AnalysisActions,
    go_toolchain: GoToolchainInfo,
    pkg_name: str,
    pkg_import_path: str,
    go_files: list[Artifact],
    cgo_files: list[Artifact],
    coverage_enabled: bool,
    coverage_mode: GoCoverageMode | None,
) -> (list[Artifact], list[Artifact], Artifact | None):
    if not coverage_enabled or coverage_mode == None:
        return go_files, cgo_files, None

    if len(go_files) + len(cgo_files) == 0:
        return go_files, cgo_files, None

    env = get_toolchain_env_vars(go_toolchain)

    cover_meta_file = actions.declare_output("cover_meta.bin", has_content_based_path = True)
    out_config_file = actions.declare_output("out_config.json", has_content_based_path = True)

    # Based on https://pkg.go.dev/cmd/internal/cov/covcmd#CoverPkgConfig
    pkgcfg = {
        "EmitMetaFile": cover_meta_file.as_output(),
        "Granularity": "perblock",
        "Local": True,
        "ModulePath": "",
        "OutConfig": out_config_file.as_output(),
        "PkgName": pkg_name,
        "PkgPath": pkg_import_path,
    }
    pkgcfg_file = actions.write_json("pkg.cfg", pkgcfg, has_content_based_path = True)

    # Grab the first 16 characters of sha256.
    # This is sufficient to make the coverage variable unique enough
    # while keeping the instrumented file a bit more readable.
    var = "GoCover_" + sha256(pkg_import_path)[:16]
    instrum_vars_file = actions.declare_output("with_instrumentation", "instrum_vars.go", has_content_based_path = True)
    instrum_go_files = [actions.declare_output("with_instrumentation", go_file.short_path, has_content_based_path = True) for go_file in go_files]
    instrum_cgo_files = [actions.declare_output("with_instrumentation", cgo_file.short_path, has_content_based_path = True) for cgo_file in cgo_files]
    instrum_all_files = [instrum_vars_file] + instrum_go_files + instrum_cgo_files
    outfilelist = actions.write("outfilelist.txt", cmd_args([f.as_output() for f in instrum_all_files], ""), has_content_based_path = True)

    cover_cmd = [
        go_toolchain.cover,
        ["-mode", coverage_mode.value],
        ["-var", var],
        cmd_args(["-outfilelist", outfilelist], hidden = [f.as_output() for f in instrum_all_files]),
        cmd_args(["-pkgcfg", pkgcfg_file], hidden = [cover_meta_file.as_output(), out_config_file.as_output()]),
        go_files,
        cgo_files,
    ]

    actions.run(cover_cmd, env = env, category = "go_cover", identifier = pkg_import_path, allow_cache_upload = go_toolchain.allow_cache_upload)

    return [instrum_vars_file] + instrum_go_files, instrum_cgo_files, out_config_file
