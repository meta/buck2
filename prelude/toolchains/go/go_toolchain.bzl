# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//go:toolchain.bzl", "GoDistrInfo", "GoToolchainInfo", "parse_go_version")

def _go_toolchain_impl(ctx):
    # Note: It makes sense to make GoDirstrInfo an attribute of GoToolchainInfo.
    # That's a breaking change, so we'll need to notify oss users.
    go_distr = ctx.attrs.go_distr[GoDistrInfo]
    return [
        DefaultInfo(
            sub_targets = {
                "go": [
                    RunInfo(
                        cmd_args(
                            ctx.attrs.go_wrapper[RunInfo],
                            ["--go", go_distr.bin_go],
                            ["--goroot", go_distr.go_root],
                            ["--default-goarch", ctx.attrs.env_go_arch],
                            ["--default-goos", ctx.attrs.env_go_os],
                            "--",
                        )
                    ),
                ]
            },
        ),
        GoToolchainInfo(
            allow_cache_upload = ctx.attrs.allow_cache_upload,
            assembler = go_distr.tool_asm,
            assembler_flags = ctx.attrs.assembler_flags,
            cxx_compiler_flags = ctx.attrs.cxx_compiler_flags,
            cgo = go_distr.tool_cgo,
            compiler = go_distr.tool_compile,
            compiler_flags = ctx.attrs.compiler_flags,
            pkg_analyzer = ctx.attrs.pkg_analyzer[RunInfo],
            gen_embedcfg = ctx.attrs.gen_embedcfg[RunInfo],
            external_linker_flags = ctx.attrs.external_linker_flags,
            go_wrapper = ctx.attrs.go_wrapper[RunInfo],
            cover = go_distr.tool_cover,
            go = go_distr.bin_go,
            env_go_arch = ctx.attrs.env_go_arch,
            env_go_os = ctx.attrs.env_go_os,
            env_go_arm = ctx.attrs.env_go_arm,
            env_go_root = go_distr.go_root,
            env_go_debug = ctx.attrs.env_go_debug,
            env_go_experiment = ctx.attrs.env_go_experiment,
            linker = go_distr.tool_link,
            linker_flags = ctx.attrs.linker_flags,
            packer = ctx.attrs.tool_pack[RunInfo],
            build_tags = ctx.attrs.build_tags,
            asan = ctx.attrs.asan,
            race = ctx.attrs.race,
            fuzz = ctx.attrs.fuzz,
            version = go_distr.version,
        ),
    ]

go_toolchain = rule(
    impl = _go_toolchain_impl,
    is_toolchain_rule = True,
    attrs = {
        "allow_cache_upload": attrs.option(attrs.bool(), default = None),
        "asan": attrs.bool(default = False),
        "assembler_flags": attrs.list(attrs.arg(), default = []),
        "build_tags": attrs.list(attrs.string(), default = []),
        "compiler_flags": attrs.list(attrs.arg(), default = []),
        "cxx_compiler_flags": attrs.list(attrs.arg(), default = []),
        "env_go_arch": attrs.string(),
        "env_go_arm": attrs.option(attrs.string(), default = None),
        "env_go_debug": attrs.dict(attrs.string(), attrs.string(), default = {}),
        "env_go_experiment": attrs.list(attrs.string(), default = []),
        "env_go_os": attrs.string(),
        "external_linker_flags": attrs.list(attrs.arg(), default = []),
        "fuzz": attrs.bool(default = False),
        "gen_embedcfg": attrs.exec_dep(providers = [RunInfo], default = "prelude//go/tools:gen_embedcfg"),
        "go_distr": attrs.exec_dep(providers = [GoDistrInfo]),
        "go_wrapper": attrs.exec_dep(providers = [RunInfo], default = "prelude//go/tools:go_wrapper"),
        "linker_flags": attrs.list(attrs.arg(), default = []),
        "pkg_analyzer": attrs.exec_dep(providers = [RunInfo], default = "prelude//go/tools:pkg_analyzer"),
        "race": attrs.bool(default = False),
        "tool_pack": attrs.exec_dep(providers = [RunInfo], default = "prelude//go/tools:tool_pack"),
    },
)

def _go_distr_impl(ctx):
    go_root = ctx.attrs.go_root
    go_os, go_arch = ctx.attrs.go_os_arch
    suffix = ".exe" if go_os == "windows" else ""
    go_platform = "{}_{}".format(go_os, go_arch)
    tool_prefix = "pkg/tool/" + go_platform
    bin_prefix = "bin/{}".format(go_platform) if ctx.attrs.multiplatform else "bin"
    return [
        DefaultInfo(),
        GoDistrInfo(
            bin_go = RunInfo(go_root.project(bin_prefix + "/go" + suffix)),
            go_root = go_root,
            tool_asm = RunInfo(go_root.project(tool_prefix + "/asm" + suffix)),
            tool_compile = RunInfo(go_root.project(tool_prefix + "/compile" + suffix)),
            tool_cover = RunInfo(go_root.project(tool_prefix + "/cover" + suffix)),
            tool_cgo = RunInfo(go_root.project(tool_prefix + "/cgo" + suffix)),
            tool_link = RunInfo(go_root.project(tool_prefix + "/link" + suffix)),
            version = parse_go_version(ctx.attrs.version),
        ),
    ]

go_distr = rule(
    impl = _go_distr_impl,
    attrs = {
        "go_os_arch": attrs.tuple(attrs.string(), attrs.string()),
        "go_root": attrs.source(allow_directory = True),
        "multiplatform": attrs.bool(default = False),
        "version": attrs.string(),
    },
)
