# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Helper rule which introduces proc macros into the dependency graph

load(
    ":link_info.bzl",
    "RustProcMacroArchInfo",
    "RustProcMacroArchVariant",
    "RustProcMacroInfo",
    "RustProcMacroMarker",
    "RustProcMacroPlugin",
)
load(":rust_toolchain.bzl", "require_rust_arch_variants")

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
    },
)

def _arch_aggregate_impl(ctx: AnalysisContext) -> list[Provider]:
    variants = require_rust_arch_variants(ctx.attrs.arch_variants, ctx.label)
    crate = None
    arch_info = {}
    outputs = []
    for arch, dep in variants.items():
        info = dep[RustProcMacroInfo]
        if info.crate.dynamic != None:
            fail("{} dual-arch proc-macro aggregate does not support dynamic crate names".format(ctx.label))
        if crate != None and crate.simple != info.crate.simple:
            fail("{} proc-macro arch variants have different crate identities".format(ctx.label))
        crate = info.crate
        outputs.append(info.dylib)
        arch_info[arch] = RustProcMacroArchVariant(
            dylib = info.dylib,
            singleton_tset = info.singleton_tset,
        )
    return [
        DefaultInfo(default_outputs = outputs),
        RustProcMacroArchInfo(crate = crate, arch_variants = arch_info),
    ]

rust_proc_macro_arch_aggregate = rule(
    impl = _arch_aggregate_impl,
    attrs = {
        "arch_variants": attrs.dict(
            key = attrs.string(),
            value = attrs.dep(providers = [RustProcMacroInfo]),
        ),
    },
)
