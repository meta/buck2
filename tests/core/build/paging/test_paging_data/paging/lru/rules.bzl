# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("//:rules.bzl", "MyInfo")

Payload = provider(fields = ["items"])
_SHARED = struct(items = ["a", "b", "c"])

def _memory_node_impl(ctx):
    # Independent payloads make each cold analysis reclaimable while a hot one
    # still owns the shared module heap. MyInfo keeps audit output small.
    items = ["{}:{}:".format(ctx.label.name, i) + "x" * 200 for i in range(1000)]
    return [DefaultInfo(), MyInfo(value = _SHARED), Payload(items = items)]

memory_node = rule(impl = _memory_node_impl, attrs = {})

def _memory_reader_impl(ctx):
    items = ctx.attrs.dep[Payload].items
    for i, item in enumerate(items):
        expected = "{}:{}:".format(ctx.attrs.dep.label.name, i) + "x" * 200
        if item != expected:
            fail("Payload item {}: expected {}, got {}".format(i, expected, item))
    return [DefaultInfo(), MyInfo(value = len(items))]

memory_reader = rule(
    impl = _memory_reader_impl,
    attrs = {"dep": attrs.dep(providers = [Payload])},
)
