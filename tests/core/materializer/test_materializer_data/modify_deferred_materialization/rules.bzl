# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _requires_local(ctx):
    for label in ctx.attrs.labels:
        if label == "buck2_test_local_exec":
            return True
    return False

def _proto_genrule_impl(ctx):
    out_artifact = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)
    env_vars = {
        "OUT": cmd_args(out_artifact.as_output()),
    }
    ctx.actions.run(
        cmd_args(["fbpython", "-c", ctx.attrs.python]),
        env = env_vars,
        prefer_local = _requires_local(ctx),
        category = "genrule",
    )
    return [DefaultInfo(default_output = out_artifact)]

proto_genrule = rule(
    impl = _proto_genrule_impl,
    attrs = {
        "labels": attrs.list(attrs.string(), default = []),
        "out": attrs.string(),
        "python": attrs.option(attrs.arg(), default = None),
    },
)
