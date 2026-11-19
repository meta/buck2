# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":tr.bzl", "iphone_to_watch_transition")

def _nop_op(*_args, **_kwargs):
    fail("this is cquery only test, no rules are executed")

my_little_iphone_binary = rule(
    impl = _nop_op,
    attrs = {
        "watch_resource": attrs.transition_dep(cfg = iphone_to_watch_transition),
    },
)

my_resource = rule(impl = _nop_op, attrs = {})

my_alias = rule(
    impl = _nop_op,
    attrs = {
        "to": attrs.dep(),
    },
)
