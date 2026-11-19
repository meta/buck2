# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":go_list.bzl", "GoListOut")
load(":toolchain.bzl", "GoToolchainInfo", "get_toolchain_env_vars")

def go_list_stdlib(actions: AnalysisActions, go_toolchain: GoToolchainInfo, cgo_enabled: bool) -> Artifact:
    env = get_toolchain_env_vars(go_toolchain)

    if go_toolchain.env_go_root == None:
        fail("GOROOT has to be set ahead of time for native stdlib build")

    # Hack: cut out redundant dependencies as we only care about "src" dir
    env["GOROOT"] = cmd_args(go_toolchain.env_go_root.project("src"), parent = 1)

    env["CGO_ENABLED"] = "1" if cgo_enabled else "0"

    go_list_stdlib_out = actions.declare_output("go_list_stdlib.txt", has_content_based_path = True)
    go_list_args = [
        go_toolchain.go_wrapper,
        ["--go", go_toolchain.go],
        ["--output", go_list_stdlib_out.as_output()],
        "--convert-json-stream",
        "list",
        "-json=ImportPath,ImportMap,EmbedFiles,Name,Imports,GoFiles,HFiles,CFiles,CXXFiles,CgoFiles,SFiles,SysoFiles,CgoCFLAGS,CgoCPPFLAGS,IgnoredGoFiles,IgnoredOtherFiles,EmbedPatterns",
        ["-tags", ",".join(go_toolchain.build_tags)] if go_toolchain.build_tags else [],
        ["-race"] if go_toolchain.race and cgo_enabled else [],
        ["-asan"] if go_toolchain.asan and cgo_enabled else [],
        "std",
    ]
    actions.run(go_list_args, env = env, category = "go_list_stdlib", allow_cache_upload = go_toolchain.allow_cache_upload)

    return go_list_stdlib_out

StdGoListOut = record(
    import_path = field(str),
    import_map = field(dict[str, str]),
    embed_files = field(list[Artifact]),
    go_list = field(GoListOut),
)

def parse_go_list_stdlib_out(goroot: Artifact, go_list: dict) -> StdGoListOut:
    import_path = go_list.get("ImportPath", "")

    def project_files(files: list[str]) -> list[Artifact]:
        return [goroot.project("src/" + import_path + "/" + f) for f in files]

    h_files = go_list.get("HFiles", [])
    if import_path == "runtime":
        # Hack: `go list` doesn't provide information about this dependency
        # but we have to declare it to generate -I flag correctly,
        # this wouldn't work if these files were not in a sub-directory.
        h_files.extend([
            "cgo/abi_amd64.h",
            "cgo/abi_loong64.h",
            "cgo/abi_arm64.h",
            "cgo/abi_ppc64x.h",
        ])

    return StdGoListOut(
        import_path = import_path,
        import_map = go_list.get("ImportMap", {}),
        embed_files = project_files(go_list.get("EmbedFiles", [])),
        go_list = GoListOut(
            name = go_list.get("Name", ""),
            imports = go_list.get("Imports", []),
            go_files = project_files(go_list.get("GoFiles", [])),
            h_files = project_files(h_files),
            c_files = project_files(go_list.get("CFiles", [])),
            cxx_files = project_files(go_list.get("CXXFiles", [])),
            cgo_files = project_files(go_list.get("CgoFiles", [])),
            s_files = project_files(go_list.get("SFiles", [])),
            syso_files = project_files(go_list.get("SysoFiles", [])),
            cgo_cflags = go_list.get("CgoCFLAGS", []),
            cgo_cppflags = go_list.get("CgoCPPFLAGS", []),
            ignored_go_files = project_files(go_list.get("IgnoredGoFiles", [])),
            ignored_other_files = project_files(go_list.get("IgnoredOtherFiles", [])),
            embed_patterns = go_list.get("EmbedPatterns", []),
        ),
    )
