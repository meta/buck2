# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args("cp", ctx.attrs.src, out.as_output()),
        category = "run",
    )
    return [DefaultInfo(default_output = out, sub_targets = {"src": [DefaultInfo(default_output = ctx.attrs.src)]})]

copy_src = rule(
    impl = _impl,
    attrs = {
        "src": attrs.source(),
    },
)

def _nondeterministic_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            'import uuid,sys; open(sys.argv[2],"w").write(uuid.uuid4().hex + "\\n" + open(sys.argv[1]).read())',
            ctx.attrs.src,
            out.as_output(),
        ),
        category = "run",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

nondeterministic_src = rule(
    impl = _nondeterministic_impl,
    attrs = {
        "src": attrs.source(),
    },
)
