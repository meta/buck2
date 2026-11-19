# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:test_common.bzl", "test_common")
load("@prelude//julia:julia_toolchain.bzl", "julia_toolchain")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")
load(":julia_binary.bzl", "build_julia_command")

def julia_test_impl(ctx: AnalysisContext) -> list[Provider]:
    cmd, json_info_file = build_julia_command(ctx)
    external_runner_test_info = ExternalRunnerTestInfo(
        type = "julia",
        command = [cmd],
        contacts = ctx.attrs.contacts,
        # FIXME: Consider setting to true
        run_from_project_root = False,
    )

    return inject_test_run_info(ctx, external_runner_test_info) + [DefaultInfo(default_output = json_info_file)]

julia_test = prelude_rule(
    name = "julia_test",
    impl = julia_test_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
        "julia_args": attrs.list(attrs.string(), default = []),
        "julia_flags": attrs.list(attrs.string(), default = []),
        "main": attrs.string(),
        "srcs": attrs.list(attrs.source(), default = []),
        "_julia_toolchain": julia_toolchain(),
        # TODO: coverage
    }
    | buck.labels_arg()
    | buck.contacts_arg()
    | buck.inject_test_env_arg()
    | test_common.attributes(),
)
