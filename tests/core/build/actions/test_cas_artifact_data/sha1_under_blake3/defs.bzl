# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _cas_artifact_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.cas_artifact(
        out.as_output(),
        ctx.attrs.digest,
        ctx.attrs.use_case,
        expires_after_timestamp = 0,
        has_content_based_path = False,
    )
    return [DefaultInfo(default_output = out)]

# A file `cas_artifact` whose digest and use case come from the test.
cas_artifact = rule(
    impl = _cas_artifact_impl,
    attrs = {
        "digest": attrs.string(),
        "use_case": attrs.string(default = "buck2-testing"),
    },
)

def _source_file_impl(ctx):
    return [DefaultInfo(default_output = ctx.attrs.src)]

# Exposes a source file as a target's output, so that a remote action can consume it.
source_file = rule(
    impl = _source_file_impl,
    attrs = {
        "src": attrs.source(),
    },
)

def _consume_impl(ctx):
    out = ctx.actions.declare_output("copy.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["cp", ctx.attrs.dep[DefaultInfo].default_outputs[0], out.as_output()]),
        category = "consume",
    )
    return [DefaultInfo(default_output = out)]

# Reads `dep`'s output in an action. Run remotely, that uploads the output to the CAS under the
# command's use case.
consume = rule(
    impl = _consume_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)
