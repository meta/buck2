# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _noisy_rule_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    script = ctx.actions.write(
        "cmd.sh",
        """\
#!/bin/bash
for i in $(seq 1 10); do
    echo "stderr line $i: this is some noisy output from the action" >&2
done
mkdir -p "$(dirname "$1")"
touch "$1"
""",
        is_executable = True,
        has_content_based_path = False,
    )
    ctx.actions.run(
        [script, out.as_output()],
        always_print_stderr = True,
        category = "noisy",
        identifier = ctx.attrs.name,
    )
    return [DefaultInfo(default_output = out)]

noisy_rule = rule(impl = _noisy_rule_impl, attrs = {})

_noisy_test_script = """
import sys
if '--list' in sys.argv:
    print('noisy_test')
    sys.exit(0)
for i in range(10):
    print(f'test output line {i}: this is some noisy output from the test', file=sys.stderr)
sys.exit(1)
"""

def _noisy_test_rule_impl(ctx):
    out = ctx.actions.declare_output("file", has_content_based_path = False)
    ctx.actions.run(["touch", out.as_output()], category = "touch")
    return [
        DefaultInfo(out),
        ExternalRunnerTestInfo(
            command = ["fbpython", "-c", _noisy_test_script],
            use_project_relative_paths = True,
            type = "custom",
        ),
    ]

noisy_test_rule = rule(impl = _noisy_test_rule_impl, attrs = {})
