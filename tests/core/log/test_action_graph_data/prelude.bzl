# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write(ctx):
    return [DefaultInfo(default_output = ctx.actions.write("a.txt", "a"))]

write = rule(impl = _write, attrs = {})

def _copy(ctx):
    src = ctx.attrs.src[DefaultInfo].default_outputs[0]
    return [DefaultInfo(default_output = ctx.actions.copy_file("b.txt", src))]

copy = rule(impl = _copy, attrs = {"src": attrs.dep()})

def _fail(ctx):
    out = ctx.actions.declare_output("never")
    ctx.actions.run(cmd_args(["sh", "-c", "exit 1", out.as_output()]), category = "fail", local_only = True)
    return [DefaultInfo(default_output = out)]

fail = rule(impl = _fail, attrs = {})
