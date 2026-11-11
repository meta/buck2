# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Helper rule which introduces proc macros into the dependency graph

load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(
    ":link_info.bzl",
    "RustProcMacroArchInfo",
    "RustProcMacroArchVariant",
    "RustProcMacroInfo",
    "RustProcMacroMarker",
    "RustProcMacroPlugin",
)
load(":rust_toolchain.bzl", "require_rust_arch_variants")

def _impl(ctx):
    # FIXME(JakobDegen): The rules themselves do not need any of the exec configured providers.
    # They only ever access the `RustProcMacroMarker` provider and do everything else through
    # plugins. However, we cannot return just that provider, since it results in
    # `buck2 build :proc_macro` building nothing. We would get the right behavior out of the
    # command line build invocation by taking a regular target dep on the aliased target and
    # returning those targets. However, that also does not work because it would result the target
    # configuration for a Rust library needing to be compatible with the target configuration for
    # its proc macro deps (a Linux-only proc macro should still be usable in a Rust build targeting
    # Mac).
    #
    # Instead, we take an exec dep on the aliased target and return those providers. This is a
    # compromise solution. The upside is that it avoids introducing any unnecessary compatibility
    # constraints. The downside is that it's still slightly wrong - because of the exec transition,
    # the returned artifacts won't correctly obey opt level/sanitizer/platform constraints in the
    # target configuration. However, this matches the behavior in fbsource today.
    #
    # Once config modifiers are implemented and roll out, it should not be too hard to get the fully
    # correct behavior
    providers = list(ctx.attrs.actual_exec.providers)
    exec_arch_info = ctx.attrs.actual_exec.get(RustProcMacroArchInfo)
    if exec_arch_info != None:
        require_rust_arch_variants(exec_arch_info.arch_variants, ctx.label)
        if ctx.attrs.actual_exec.label.raw_target() != ctx.attrs.actual_plugin:
            fail("{} dual-arch proc macro must use the same logical target for actual_exec and actual_plugin".format(ctx.label))
    providers.append(RustProcMacroMarker(label = ctx.attrs.actual_plugin))
    return providers

rust_proc_macro_alias = rule(
    impl = _impl,
    attrs = {
        "actual_exec": attrs.exec_dep(),
        "actual_plugin": attrs.plugin_dep(kind = RustProcMacroPlugin),
        "_rust_toolchain": toolchains_common.rust(),
    },
)

def _arch_aggregate_impl(ctx: AnalysisContext) -> list[Provider]:
    variants = require_rust_arch_variants(ctx.attrs.arch_variants, ctx.label)
    crate = None
    arch_info = {}
    outputs = []
    for arch, dep in variants.items():
        info = dep[RustProcMacroInfo]
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
