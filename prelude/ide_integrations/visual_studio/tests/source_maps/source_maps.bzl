# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _source_map_impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo()]

source_keyed_srcs = rule(
    impl = _source_map_impl,
    attrs = {
        "srcs": attrs.dict(key = attrs.source(), value = attrs.option(attrs.source(), default = None)),
    },
)

named_srcs = rule(
    impl = _source_map_impl,
    attrs = {
        "srcs": attrs.dict(key = attrs.string(), value = attrs.source()),
    },
)
