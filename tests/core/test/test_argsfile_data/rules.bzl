# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# The `type` picks a tpx framework translator, and only a translator that lists
# cases and then passes them to the test produces arguments an argsfile can
# hold. `lionhead` is the cheapest of those to satisfy -- list one name per line
# on `--list`, take them back as `--tests <cases>` -- which is why other
# fixtures here use it too. `custom`, the usual choice in this directory, has no
# cases at all, and the translators that pass every case in one command expect
# the test to report a result per case, which is machinery unrelated to what
# this is testing.
#
# `fbpython` is the program but does not understand `@file`; this script does.
# That split is why the runner, not Buck, decides what may move into an
# argsfile. The script fails the test case unless it is handed its case exactly
# the way `EXPECT_ARGSFILE` says, so a command that stopped being expanded
# through `push_argsfile_arg` surfaces as a test failure instead of passing
# quietly.
script = """
import os
import sys

args = sys.argv[1:]
if '--list' in args:
    print('case_one')
    sys.exit(0)

expect = os.environ['EXPECT_ARGSFILE']
cases = args[args.index('--tests') + 1:]
if expect == 'none':
    if any(case.startswith('@') for case in cases):
        sys.stderr.write('expected no argsfile, got %r\\n' % (cases,))
        sys.exit(1)
else:
    if len(cases) != 1 or not cases[0].startswith('@'):
        sys.stderr.write('expected a single argsfile, got %r\\n' % (cases,))
        sys.exit(1)
    path = cases[0][1:]
    if (expect == 'absolute') != os.path.isabs(path):
        sys.stderr.write('expected an %s path, got %r\\n' % (expect, path))
        sys.exit(1)
    with open(path) as f:
        cases = [line for line in f.read().split('\\n') if line]

if cases != ['case_one']:
    sys.stderr.write('got cases %r\\n' % (cases,))
    sys.exit(1)

sys.exit(0)
"""

def _argsfile_test_impl(ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            command = ["fbpython", "-c", script],
            # A suite that cannot take project-relative paths is the reason
            # `ExpandableArg::Argsfile` carries a path rather than a rendered
            # string: the `@<path>` is only made absolute because the command
            # line builder expands it like any other path.
            use_project_relative_paths = ctx.attrs.expect_argsfile != "absolute",
            type = "lionhead",
            labels = ctx.attrs.labels,
            env = {"EXPECT_ARGSFILE": ctx.attrs.expect_argsfile},
        ),
    ]

argsfile_test = rule(
    attrs = {
        "expect_argsfile": attrs.enum(["none", "relative", "absolute"], default = "none"),
        "labels": attrs.list(attrs.string(), default = []),
    },
    impl = _argsfile_test_impl,
)
