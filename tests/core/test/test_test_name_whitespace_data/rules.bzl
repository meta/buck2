# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    name = ctx.attrs.test_name
    status = ctx.attrs.status
    return [
        DefaultInfo(),
        InternalRunnerTestInfo(
            type = "custom",
            listing_command = ["fbpython", "-c", ""],
            command = ["fbpython", "-c", ""],
            parse_test_listing = lambda _stdout: [{"filter": "case", "name": "case"}],
            parse_test_result = lambda _stdout, _stderr, _exit_code: [{"name": name, "status": status}],
        ),
    ]

# A test target with one test case, reported under `test_name` with `status`.
named_test = rule(
    impl = _impl,
    attrs = {
        "status": attrs.string(),
        "test_name": attrs.string(),
    },
)
