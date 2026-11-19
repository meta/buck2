# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# The lambda declares the action that produces `out` with `out` itself as an
# input. Nothing rejects that: by the time the lambda runs, `out` is bound (to
# the dynamic action), so it passes for an ordinary input.
def _self_impl(ctx):
    trigger = ctx.actions.write("trigger", "")
    out = ctx.actions.declare_output("out")

    def f(ctx, _artifacts, outputs):
        ctx.actions.copy_file(outputs[out], out)

    ctx.actions.dynamic_output(dynamic = [trigger], inputs = [], outputs = [out.as_output()], f = f)
    return [DefaultInfo(default_output = out)]

self_cycle = rule(impl = _self_impl, attrs = {})

# The cycle runs through a lambda's `dynamic` input: `a`'s lambda reads `b`, and
# `b` is produced by an action, declared in another lambda, that consumes `a`.
def _through_lambda_impl(ctx):
    trigger = ctx.actions.write("trigger", "")
    a = ctx.actions.declare_output("a")
    b = ctx.actions.declare_output("b")

    def fb(ctx, _artifacts, outputs):
        ctx.actions.copy_file(outputs[b], a)

    ctx.actions.dynamic_output(dynamic = [trigger], inputs = [], outputs = [b.as_output()], f = fb)

    def fa(ctx, artifacts, outputs):
        ctx.actions.write(outputs[a], artifacts[b].read_string())

    ctx.actions.dynamic_output(dynamic = [b], inputs = [], outputs = [a.as_output()], f = fa)
    return [DefaultInfo(default_output = a)]

through_lambda_cycle = rule(impl = _through_lambda_impl, attrs = {})
