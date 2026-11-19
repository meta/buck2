# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

simple = rule(impl = lambda _ctx: [DefaultInfo()], attrs = {})

consumer = rule(
    impl = lambda _ctx: [DefaultInfo()],
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
    },
)

# `attrs.source()` accepts either a path or a label, so it has its own coercion
# logic and needs its own coverage for eponymous labels.
src_consumer = rule(
    impl = lambda _ctx: [DefaultInfo()],
    attrs = {
        "srcs": attrs.list(attrs.source(), default = []),
    },
)
