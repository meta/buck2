# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":transitions.bzl", "force_opt_mode", "force_opt_mode_v2")

BuildModeInfo = provider(
    fields = {
        "mode": provider_field(str),
    },
)

def _unified_foo_impl(ctx):
    output = ctx.actions.declare_output("output.txt", has_content_based_path = False)
    ctx.actions.write(output, "hello world!")

    return [DefaultInfo(default_output = output)]

unified_foo = rule(
    impl = _unified_foo_impl,
    attrs = {
        "cpu": attrs.string(),
        "os": attrs.string(),
        "sanitizer_name": attrs.option(attrs.string()),
    },
)

def _unified_build_mode(ctx):
    return [DefaultInfo(), BuildModeInfo(mode = ctx.attrs.build_mode)]

unified_build_mode = rule(
    impl = _unified_build_mode,
    attrs = {
        "build_mode": attrs.string(),
    },
)

TransitionBuildModeInfo = provider(
    fields = {
        "dep": provider_field(str),
        "mode": provider_field(str),
        "opt_dep": provider_field(str),
    },
)

def _unified_with_transition_impl(ctx: AnalysisContext) -> list[Provider]:
    build_mode = ctx.attrs.build_mode

    dep_build_mode = ctx.attrs.dep[BuildModeInfo].mode
    opt_dep_build_mode = ctx.attrs.opt_dep[BuildModeInfo].mode

    output = ctx.actions.declare_output("output.txt", has_content_based_path = False)

    content = "self: {}\ndep: {}\nopt_dep:{}\n".format(build_mode, dep_build_mode, opt_dep_build_mode)

    ctx.actions.write(output, content)

    return [DefaultInfo(default_output = output), TransitionBuildModeInfo(mode = build_mode, dep = dep_build_mode, opt_dep = opt_dep_build_mode)]

unified_with_transition = rule(
    impl = _unified_with_transition_impl,
    attrs = {
        "build_mode": attrs.string(),
        "dep": attrs.dep(),
        "opt_dep": attrs.transition_dep(cfg = force_opt_mode),
    },
)

unified_with_transition_v2 = rule(
    impl = _unified_with_transition_impl,
    attrs = {
        "build_mode": attrs.string(),
        "dep": attrs.dep(),
        "opt_dep": attrs.transition_dep(cfg = force_opt_mode_v2),
    },
)
