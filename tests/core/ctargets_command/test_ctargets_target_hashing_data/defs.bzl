# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo()]

dummy = rule(
    attrs = {
        "configured_deps": attrs.list(attrs.configured_dep(), default = []),
        "deps": attrs.list(attrs.dep(), default = []),
        "left": attrs.option(attrs.configured_dep(), default = None),
        "right": attrs.option(attrs.configured_dep(), default = None),
        "srcs": attrs.list(attrs.source(), default = []),
        "value": attrs.string(default = ""),
    },
    impl = _impl,
)

def _split_impl(platform: PlatformInfo, refs: struct) -> dict[str, PlatformInfo]:
    _ignore = platform  # buildifier: disable=unused-variable
    return {
        "left": refs.left[PlatformInfo],
        "right": refs.right[PlatformInfo],
    }

_swap_platforms = read_config("test", "swap_platforms", "false") == "true"

_split = transition(
    impl = _split_impl,
    refs = {
        "left": "root//:macos_platform" if _swap_platforms else "root//:linux_platform",
        "right": "root//:linux_platform" if _swap_platforms else "root//:macos_platform",
    },
    split = True,
)

split_consumer = rule(
    attrs = {
        "dep": attrs.split_transition_dep(cfg = _split),
    },
    impl = _impl,
)

def _fixed_platform_impl(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    _ignore = platform  # buildifier: disable=unused-variable
    return refs.platform[PlatformInfo]

_fixed_platform = transition(
    impl = _fixed_platform_impl,
    refs = {"platform": "root//:platform"},
)

transitioned = rule(
    attrs = {"value": attrs.string(default = "")},
    impl = _impl,
    cfg = _fixed_platform,
)
