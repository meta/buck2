# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""A rule that applies the android_binary CPU transition to a dependency.

This is useful for targets that need to be configured with the same CPU
platform as an android_binary's deps, but are referenced from genrules
that run in the binary's parent (arch-agnostic) configuration.
"""

load("@prelude//android:configuration.bzl", "CPU_TRANSITION_ATTRS", "cpu_transition")
load("@prelude//decls:common.bzl", "TargetCpuType")

def _android_cpu_transitioned_dep_impl(ctx: AnalysisContext) -> list[Provider]:
    return ctx.attrs.dep.providers

android_cpu_transitioned_dep = rule(
    impl = _android_cpu_transitioned_dep_impl,
    attrs = {
        "cpu_filters": attrs.list(attrs.enum(TargetCpuType), default = []),
        "dep": attrs.transition_dep(cfg = cpu_transition),
        "min_sdk_version": attrs.option(attrs.int(), default = None),
    }
    | CPU_TRANSITION_ATTRS,
)
