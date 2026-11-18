# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _lib_impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo()]

lib = rule(impl = _lib_impl, attrs = {})

def _shadow_impl(ctx: AnalysisContext) -> list[Provider]:
    # A configuration-based output path: it is named by the anon target's hash.
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.write(out, str(ctx.attrs.dep))
    return [DefaultInfo(default_output = out)]

# The documented pattern: one anon target per dep, default name, differing only in a label.
_shadow = rule(impl = _shadow_impl, attrs = {"dep": attrs.label()})

def _fanout_impl(ctx: AnalysisContext) -> Promise:
    def f(providers_list) -> list[Provider]:
        outs = [p[DefaultInfo].default_outputs[0] for p in providers_list]
        return [DefaultInfo(default_outputs = outs)]

    return ctx.actions.anon_targets([(_shadow, {"dep": d.label.raw_target()}) for d in ctx.attrs.deps]).promise.map(f)

fanout = rule(impl = _fanout_impl, attrs = {"deps": attrs.list(attrs.dep())})
