# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_BODY = """
import os
from pathlib import Path
Path(os.environ["TEST_BODY_SENTINEL"]).write_text("executed")
"""

def _test_case(ctx):
    env = {"TEST_BODY_SENTINEL": read_config("test", "body_sentinel", "")}
    executor = CommandExecutorConfig(local_enabled = True, remote_enabled = False)
    if ctx.attrs.skipped:
        return [
            DefaultInfo(),
            InternalRunnerTestInfo(
                type = "custom",
                listing_command = ["fbpython", "-c", ""],
                command = ["fbpython", "-c", _BODY + '\nprint("SKIP")'],
                env = env,
                default_executor = executor,
                parse_test_listing = lambda _stdout: [{"filter": "case", "name": "case"}],
                parse_test_result = lambda stdout, _stderr, exit_code: [
                    {"name": "case", "status": "SKIP" if exit_code == 0 and stdout.strip() == "SKIP" else "FAIL"}
                ],
            ),
        ]
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            type = "custom",
            command = ["fbpython", "-c", _BODY],
            labels = ctx.attrs.labels,
            env = env,
            default_executor = executor,
            use_project_relative_paths = True,
        ),
    ]

test_case = rule(
    impl = _test_case,
    attrs = {
        "labels": attrs.list(attrs.string(), default = []),
        "skipped": attrs.bool(default = False),
    },
)

def _no_tests(_ctx):
    return [DefaultInfo()]

no_tests = rule(impl = _no_tests, attrs = {})
