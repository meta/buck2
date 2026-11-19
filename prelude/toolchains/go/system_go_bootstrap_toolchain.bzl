# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//go_bootstrap:toolchain.bzl", "GoBootstrapToolchainInfo")
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

def _system_go_bootstrap_toolchain_impl(ctx):
    go_os, go_arch = go_platform()

    script_language = ScriptLanguage("bat" if go_os == "windows" else "sh")
    go = "go.exe" if go_os == "windows" else "go"

    go_cmd = cmd_script(ctx.actions, "go", cmd_args(go), script_language)

    return [
        DefaultInfo(),
        GoBootstrapToolchainInfo(
            env_go_arch = go_arch,
            env_go_os = go_os,
            go = RunInfo(go_cmd),
            go_wrapper = ctx.attrs.go_wrapper[RunInfo],
        ),
    ]

system_go_bootstrap_toolchain = rule(
    impl = _system_go_bootstrap_toolchain_impl,
    doc = """Example system go toolchain rules (WIP). Usage:
  system_go_bootstrap_toolchain(
      name = "go_bootstrap",
      visibility = ["PUBLIC"],
  )""",
    attrs = {
        "go_wrapper": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//go_bootstrap/tools:go_wrapper_py")),
    },
    is_toolchain_rule = True,
)
