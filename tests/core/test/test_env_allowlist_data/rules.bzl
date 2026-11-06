# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A test whose command asserts that specific environment variables were
# inherited from the daemon with specific values, and fails otherwise. Used to
# observe what the local execution env allowlist actually lets through.

_SCRIPT_PREFIX = """
import os
import sys

if "--list" in sys.argv:
    print("test1\\n")
    sys.exit(0)

failures = []

def check(name, want):
    got = os.environ.get(name)
    if got != want:
        failures.append("expected " + name + "=" + repr(want) + ", got " + repr(got))

"""

_SCRIPT_SUFFIX = """
if failures:
    sys.stderr.write("\\n".join(failures) + "\\n")
    sys.exit(1)
sys.exit(0)
"""

def _env_probe_test_impl(ctx):
    # An empty expected value means "must not be set at all".
    checks = [
        'check("{}", {})'.format(name, '"{}"'.format(want) if want else "None")
        for name, want in ctx.attrs.expect_env.items()
    ]
    script = _SCRIPT_PREFIX + "\n".join(checks) + _SCRIPT_SUFFIX

    out = ctx.actions.declare_output("file", has_content_based_path = False)
    ctx.actions.run(
        ["touch", out.as_output()],
        category = "touch",
    )
    return [
        DefaultInfo(out),
        ExternalRunnerTestInfo(
            command = ["fbpython", "-c", script],
            use_project_relative_paths = True,
            type = "lionhead",
        ),
    ]

env_probe_test = rule(
    attrs = {
        "expect_env": attrs.dict(attrs.string(), attrs.string(), default = {}),
    },
    impl = _env_probe_test_impl,
)
