# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    if ctx.attrs.fail_build:
        out = ctx.actions.declare_output("out")
        ctx.actions.run(
            ["fbpython", "-c", "import sys; print('build action executed'); sys.exit(1)", out.as_output()],
            category = "failing_build",
            local_only = True,
        )
    else:
        out = ctx.actions.write("out", "pass")
    check = ctx.actions.write("check", "checked")
    command = ["fbpython", "-c", "import sys; sys.exit(0)"]
    return [
        DefaultInfo(
            default_output = out,
            sub_targets = {
                "check": [DefaultInfo(default_output = check)],
                "custom": [DefaultInfo(default_output = out)],
            },
        ),
        RunInfo(args = command),
    ]

def _test_impl(ctx):
    return _impl(ctx) + [
        ExternalRunnerTestInfo(
            type = "custom",
            command = ["fbpython", "-c", "import sys; sys.exit(0)"],
        )
    ]

def _rust_library_impl(ctx):
    check = ctx.actions.write("check", "checked")
    return [DefaultInfo(default_output = check, sub_targets = {"check": [DefaultInfo(default_output = check)]})]

_attrs = {"fail_build": attrs.bool(default = False)}

rust_binary = rule(impl = _impl, attrs = _attrs)
rust_library = rule(impl = _rust_library_impl, attrs = {})
cxx_library = rule(impl = _impl, attrs = _attrs)
cxx_binary = rule(impl = _impl, attrs = _attrs)
rust_test = rule(impl = _test_impl, attrs = _attrs)
cxx_test = rule(impl = _test_impl, attrs = _attrs)
rust_custom = rule(impl = _impl, attrs = _attrs)
cxx_custom = rule(impl = _impl, attrs = _attrs)
plain = rule(impl = _impl, attrs = _attrs)
