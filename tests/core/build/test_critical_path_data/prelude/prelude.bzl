# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.write("out", "test", has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

write = rule(impl = _write, attrs = {})

def _cp(ctx: AnalysisContext) -> list[Provider]:
    inp = ctx.attrs.dep[DefaultInfo].default_outputs[0]
    out = ctx.actions.declare_output("out", has_content_based_path = False)

    ctx.actions.run(
        [
            "sh",
            "-c",
            'sleep "$1" && cp "$2" "$3"',
            "--",
            str(ctx.attrs.sleep),
            inp,
            out.as_output(),
        ],
        category = "cp_action",
    )
    return [DefaultInfo(default_output = out)]

cp = rule(
    impl = _cp,
    attrs = {
        "dep": attrs.dep(),
        "sleep": attrs.int(default = 0),
    },
)

def _dynamic_cp(ctx: AnalysisContext) -> list[Provider]:
    dummy = ctx.actions.write("dummy", "", has_content_based_path = False)

    inp = ctx.attrs.dep[DefaultInfo].default_outputs[0]
    out = ctx.actions.declare_output("out", has_content_based_path = False)

    def f(ctx: AnalysisContext, _artifacts, outputs):
        # NOTE: dummy doesn't show in the critical path calculation at all.
        ctx.actions.run(
            [
                "cp",
                inp,
                outputs[out].as_output(),
            ],
            category = "dynamic_cp_action",
        )

    ctx.actions.dynamic_output(dynamic = [dummy], inputs = [inp], outputs = [out.as_output()], f = f)
    return [DefaultInfo(default_output = out)]

dynamic_cp = rule(
    impl = _dynamic_cp,
    attrs = {
        "dep": attrs.dep(),
    },
)

def _dynamic_cp2(ctx: AnalysisContext) -> list[Provider]:
    ctx.actions.write("dummy", "", has_content_based_path = False)

    inp = ctx.attrs.dep[DefaultInfo].default_outputs[0]
    out = ctx.actions.declare_output("out", has_content_based_path = False)

    def f(ctx: AnalysisContext, artifacts, outputs):
        ctx.actions.write(outputs[out].as_output(), artifacts[inp].read_string())

    ctx.actions.dynamic_output(dynamic = [inp], inputs = [], outputs = [out.as_output()], f = f)
    return [DefaultInfo(default_output = out)]

dynamic_cp2 = rule(
    impl = _dynamic_cp2,
    attrs = {
        "dep": attrs.dep(),
    },
)

script = """
import sys;
import time;
if '--list' in sys.argv:
    print('test1\\n')
else:
    time.sleep(0.1) # Sleep for 100ms
sys.exit(0)
"""

def _simple_test_impl(ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            command = ["fbpython", "-c", script],
            type = "lionhead",
            env = {"seed": ctx.attrs.seed},
        ),
    ]

simple_test = rule(
    impl = _simple_test_impl,
    attrs = {"seed": attrs.string()},
)

def _proj_identity(v):
    return v

TSetForTest = transitive_set(args_projections = {"identity": _proj_identity})

TSetForTestInfo = provider(fields = ["tset"])

def _tset_write(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.write("out", str(ctx.label.name), has_content_based_path = False)
    children = [d[TSetForTestInfo].tset for d in ctx.attrs.deps]
    tset = ctx.actions.tset(TSetForTest, value = out, children = children)
    return [
        TSetForTestInfo(tset = tset),
        DefaultInfo(
            default_output = out,
            other_outputs = [tset.project_as_args("identity")],
        ),
    ]

tset_write = rule(
    impl = _tset_write,
    attrs = {
        "deps": attrs.list(attrs.dep(providers = [TSetForTestInfo]), default = []),
    },
)

# --- Anon target rules for critical path testing ---

# Inner (leaf) anon target: just writes a file, no anon target deps.
def _anon_inner_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.write("inner_out", "inner_content", has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

_anon_inner = rule(impl = _anon_inner_impl, attrs = {})

# Outer anon target: itself creates an inner anon target dependency,
# so its analysis is also split into part1/part2.
def _anon_outer_impl(ctx: AnalysisContext) -> Promise:
    inner = ctx.actions.anon_target(_anon_inner, {})

    out = ctx.actions.declare_output("outer_out")

    def _on_inner_resolved(providers):
        inner_out = providers[DefaultInfo].default_outputs[0]
        ctx.actions.run(
            ["cp", inner_out, out.as_output()],
            category = "outer_anon_cp",
        )
        return [DefaultInfo(default_output = out)]

    return inner.promise.map(_on_inner_resolved)

_anon_outer = rule(impl = _anon_outer_impl, attrs = {})

# Top-level rule: creates the outer anon target, which itself creates
# the inner anon target. This exercises nested anon target splitting:
#   uses_anon analysis[part1] → anon_outer[part1] → anon_inner → anon_outer[part2] → uses_anon analysis[part2]
def _uses_anon_impl(ctx: AnalysisContext) -> Promise:
    anon = ctx.actions.anon_target(_anon_outer, {})

    out = ctx.actions.declare_output("final_out")

    def _on_anon_resolved(providers):
        anon_out = providers[DefaultInfo].default_outputs[0]
        ctx.actions.run(
            ["cp", anon_out, out.as_output()],
            category = "post_anon_cp",
        )
        return [DefaultInfo(default_output = out)]

    return anon.promise.map(_on_anon_resolved)

uses_anon = rule(impl = _uses_anon_impl, attrs = {})

def _config_setting_impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(), ConfigurationInfo(constraints = {}, values = {})]

config_setting = rule(impl = _config_setting_impl, attrs = {})
