# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Global data that will be retained in the bzl file's heap
# This creates ~1MB of global data
GLOBAL_DATA = list(range(1000000 // 8))

def _rule_with_globals_impl(ctx):
    """
    A rule that has access to global data in a bzl file.

    This tests that memory from bzl globals is included in the sketch.
    """
    out = ctx.actions.write("out.txt", "", has_content_based_path = False)

    return [
        DefaultInfo(default_output = out),
    ]

rule_with_globals = rule(
    impl = _rule_with_globals_impl,
    attrs = {},
)
