# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//go:toolchain.bzl", "GoToolchainInfo")
load("@prelude//os_lookup:defs.bzl", "ScriptLanguage")
load("@prelude//utils:cmd_script.bzl", "cmd_script")

def go_platform() -> (str, str):
    arch = host_info().arch
    if arch.is_aarch64:
        go_arch = "arm64"
    elif arch.is_x86_64:
        go_arch = "amd64"
    else:
        fail("Unsupported go arch: {}".format(arch))

    os = host_info().os
    if os.is_macos:
        go_os = "darwin"
    elif os.is_linux:
        go_os = "linux"
    elif os.is_windows:
        go_os = "windows"
    else:
        fail("Unsupported go os: {}".format(os))

    return go_os, go_arch

def _system_go_toolchain_impl(ctx):
    go_os, go_arch = go_platform()

    script_language = ScriptLanguage("bat" if go_os == "windows" else "sh")
    go = "go.exe" if go_os == "windows" else "go"

    go_root = ctx.actions.declare_output("goroot", dir = True, has_content_based_path = True)

    # HACK: We need a physical artifact for GOROOT to project individual source files.
    # Since we don't know the user's GOROOT location at analysis time, we copy it
    # at build time. A symlink won't work because .project() requires a real directory.
    # We use a Go binary for the copy as it's more portable than shell scripts.
    ctx.actions.run([ctx.attrs.copy_goroot[RunInfo], "-o", go_root.as_output()], category = "go_copy_goroot")

    suffix = ".exe" if go_os == "windows" else ""
    tool_prefix = "pkg/tool/{}_{}".format(go_os, go_arch)

    return [
        DefaultInfo(
            sub_targets = {
                "go": [
                    RunInfo(go),
                ]
            }
        ),
        GoToolchainInfo(
            allow_cache_upload = ctx.attrs.allow_cache_upload,
            assembler = RunInfo(cmd_script(ctx.actions, "asm", cmd_args(go, "tool", "asm"), script_language)),
            cgo = RunInfo(go_root.project(tool_prefix + "/cgo" + suffix)),
            pkg_analyzer = ctx.attrs.pkg_analyzer[RunInfo],
            gen_embedcfg = ctx.attrs.gen_embedcfg[RunInfo],
            compiler = RunInfo(cmd_script(ctx.actions, "compile", cmd_args(go, "tool", "compile"), script_language)),
            cover = RunInfo(cmd_script(ctx.actions, "cover", cmd_args(go, "tool", "cover"), script_language)),
            env_go_arch = go_arch,
            env_go_os = go_os,
            env_go_root = go_root,
            external_linker_flags = [],
            go = RunInfo(cmd_script(ctx.actions, "go", cmd_args(go), script_language)),
            go_wrapper = ctx.attrs.go_wrapper[RunInfo],
            linker = RunInfo(cmd_script(ctx.actions, "link", cmd_args(go, "tool", "link"), script_language)),
            packer = RunInfo(cmd_script(ctx.actions, "pack", cmd_args(go, "tool", "pack"), script_language)),
            build_tags = [],
            linker_flags = [],
            assembler_flags = [],
            compiler_flags = [],
            version = None,  # we are unable to run `go version` during analysis time
        ),
    ]

system_go_toolchain = rule(
    impl = _system_go_toolchain_impl,
    doc = """Example system go toolchain rules (WIP). Usage:
  system_go_toolchain(
      name = "go",
      visibility = ["PUBLIC"],
  )""",
    attrs = {
        "allow_cache_upload": attrs.option(attrs.bool(), default = None),
        "copy_goroot": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//go/tools:copy_goroot")),
        "gen_embedcfg": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//go/tools:gen_embedcfg")),
        "go_wrapper": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//go/tools:go_wrapper")),
        "pkg_analyzer": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//go/tools:pkg_analyzer")),
    },
    is_toolchain_rule = True,
)
