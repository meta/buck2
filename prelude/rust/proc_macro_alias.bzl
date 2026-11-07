# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Helper rule which introduces proc macros into the dependency graph

load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(":link_info.bzl", "RustProcMacroMarker", "RustProcMacroPlugin")

def _without_top_level_marker_impl(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    marker = refs.top_level[ConstraintValueInfo].setting.label
    return PlatformInfo(
        label = platform.label,
        configuration = ConfigurationInfo(
            constraints = {setting: value for setting, value in platform.configuration.constraints.items() if setting != marker},
            values = platform.configuration.values,
        ),
    )

# Configures `actual` the same as when the proc macro is built directly.
_without_top_level_marker = transition(
    impl = _without_top_level_marker_impl,
    refs = {"top_level": "prelude//rust/constraints:proc_macro_alias_top_level[true]"},
)

def _impl(ctx):
    # `actual` is only set when the alias is built directly.
    # `prelude//rust/constraints:proc_macro_alias_top_level[true]` is one of its target modifiers,
    # and modifiers only apply to top-level targets. Without a cfg_constructor, and thus modifiers
    # not enabled, `buck2 build :proc_macro` builds nothing.
    if ctx.attrs.actual:
        providers = list(ctx.attrs.actual.providers)
    else:
        providers = [DefaultInfo()]
    providers.append(RustProcMacroMarker(label = ctx.attrs.actual_plugin))
    return providers

rust_proc_macro_alias = rule(
    impl = _impl,
    attrs = {
        "actual": attrs.option(attrs.transition_dep(cfg = _without_top_level_marker), default = None),
        "actual_plugin": attrs.plugin_dep(kind = RustProcMacroPlugin),
        "_rust_toolchain": toolchains_common.rust(),
    },
)
