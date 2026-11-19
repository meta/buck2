# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//go:attrs.bzl", "go_common")
load(":toolchain.bzl", "GoBootstrapToolchainInfo")

def go_bootstrap_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    """
    Produces a Go binary for use in prelude. Similar to `python_bootstrap_binary`
    It doesn't depend on other Go rules and uses `go build` under the hood.
    CGo is disabled minimise dependencies.
    """
    go_toolchain = ctx.attrs._go_bootstrap_toolchain[GoBootstrapToolchainInfo]

    target_is_win = go_toolchain.env_go_os == "windows"
    exe_suffix = ".exe" if target_is_win else ""
    output = ctx.actions.declare_output(ctx.label.name + exe_suffix, has_content_based_path = True)

    # Copy files, because go:embed doesn't work with symlinks
    srcs_dir = ctx.actions.copied_dir(
        "__srcs_dir__",
        {paths.relativize(src.short_path, ctx.attrs.workdir): src for src in ctx.attrs.srcs},
        has_content_based_path = True,
    )

    cmd = cmd_args([
        go_toolchain.go_wrapper,
        go_toolchain.go,
        ["--workdir", srcs_dir],
        "build",
        "-trimpath",
        ["-o", cmd_args(output.as_output(), relative_to = srcs_dir)],
        ctx.attrs.build_args,
    ])

    env = {
        "CGO_ENABLED": "0",
        "GO111MODULE": "",
        "GOARCH": go_toolchain.env_go_arch,
        "GOOS": go_toolchain.env_go_os,
        "GOTOOLCHAIN": "local",
    }

    if go_toolchain.env_go_root != None:
        env["GOROOT"] = go_toolchain.env_go_root

    ctx.actions.run(cmd, env = env, category = "go_bootstrap_binary")

    return [
        DefaultInfo(default_output = output),
        RunInfo(args = [output]),
    ]

go_bootstrap_binary = prelude_rule(
    name = "go_bootstrap_binary",
    impl = go_bootstrap_binary_impl,
    docs = """
        A `go_bootstrap_binary()` rule builds a minimal Go executable used
        during bootstrap, where the full `go_binary()` machinery is not yet
        available. It invokes the Go toolchain directly with the given
        sources and `build_args`.
    """,
    attrs = (
        go_common.srcs_arg()
        | {
            "build_args": attrs.list(attrs.string(), default = [], doc = """Package name, file names and build flags"""),
            "workdir": attrs.string(default = "", doc = """Change to subdir before running the command"""),
        }
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "_exec_os_type": buck.exec_os_type_arg(),
            "_go_bootstrap_toolchain": toolchains_common.go_bootstrap(),
        }
    ),
)
