# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

TestRun = exec_group()

def _impl(_ctx):
    return [DefaultInfo(), ExternalRunnerTestInfo(type = "custom", command = ["true"], exec_group = TestRun)]

# No platform is both macOS and remote, so `TestRun` has no compatible execution platform. The group
# has no deps, so analysis succeeds.
r = rule(
    impl = _impl,
    attrs = {
        "test_run_exec_compatible_with": attrs.exec_compatible_with(
            exec_group = TestRun,
            default = ["//config:macos", "//config:remote"],
        ),
    },
    exec_groups = [TestRun],
)

def _copying_test_impl(ctx):
    info = ctx.attrs.test[ExternalRunnerTestInfo]
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            type = info.test_type,
            command = info.command,
            default_executor = info.default_executor,
            exec_group = info.exec_group,
        ),
    ]

# Re-creates the test info of `test` field by field without declaring its group. `test`'s group
# has no compatible execution platform, so its test info records no executor.
copying_test = rule(impl = _copying_test_impl, attrs = {"test": attrs.dep()})
