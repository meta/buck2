# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _test_startswith_impl(ctx):
    flags = ctx.attrs.compiler_flags
    matching = [f for f in flags if f.startswith("-W")]
    non_matching = [f for f in flags if not f.startswith("-W")]
    macro_prefix_matching = [f for f in flags if f.startswith("-some-flag-with-macro")]

    # Verify startswith works correctly on ResolvedStringWithMacros
    if len(matching) != 3:
        fail("Expected 3 flags starting with -W, got {}".format(len(matching)))
    if len(non_matching) != 2:
        fail("Expected 2 flags not starting with -W, got {}".format(len(non_matching)))
    if len(macro_prefix_matching) != 1:
        fail("Expected 1 flags starting with -some-flag-with-macro, got {}".format(len(macro_prefix_matching)))

    output = ctx.actions.write("result.txt", "ok\n", has_content_based_path = False)
    return [DefaultInfo(default_output = output)]

test_startswith = rule(
    impl = _test_startswith_impl,
    attrs = {
        "compiler_flags": attrs.list(attrs.arg()),
    },
)

def _test_equality_impl(ctx):
    flags = ctx.attrs.compiler_flags

    # Test equality between ResolvedStringWithMacros and plain strings
    if "-Werror" not in flags:
        fail("Expected -Werror in flags")
    if "-O2" not in flags:
        fail("Expected -O2 in flags")
    if "-nonexistent" in flags:
        fail("Did not expect -nonexistent in flags")

    output = ctx.actions.write("result.txt", "ok\n", has_content_based_path = False)
    return [DefaultInfo(default_output = output)]

test_equality = rule(
    impl = _test_equality_impl,
    attrs = {
        "compiler_flags": attrs.list(attrs.arg()),
    },
)
