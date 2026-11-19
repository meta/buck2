# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

AnonDataInfo = provider(fields = ["data"])
ParentDataInfo = provider(fields = ["data", "anon"])

def _anon_rule_impl(ctx):
    """Implementation for the anon target."""

    # Allocate some memory in the anon target
    data = list(range(ctx.attrs.memory_size // 8))

    out = ctx.actions.write("out.txt", "", has_content_based_path = False)

    return [
        DefaultInfo(default_output = out),
        AnonDataInfo(data = data),
    ]

_anon_rule = rule(
    impl = _anon_rule_impl,
    attrs = {
        "memory_size": attrs.int(),
    },
)

def _rule_with_anon_impl(ctx):
    """
    A rule that uses anon targets.

    This tests that memory from anon targets is included in the parent's sketch.
    """

    # Also allocate some memory in the parent
    parent_data = list(range(ctx.attrs.memory_size // 8))

    # Create an anon target that allocates memory
    def process_anon(providers):
        out = ctx.actions.write("out.txt", "", has_content_based_path = False)

        return [
            DefaultInfo(default_output = out),
            ParentDataInfo(data = parent_data, anon = providers[AnonDataInfo]),
        ]

    return ctx.actions.anon_target(
        _anon_rule,
        {
            "memory_size": ctx.attrs.anon_memory_size,
        },
    ).promise.map(process_anon)

rule_with_anon = rule(
    impl = _rule_with_anon_impl,
    attrs = {
        "anon_memory_size": attrs.int(),
        "memory_size": attrs.int(),
    },
)
