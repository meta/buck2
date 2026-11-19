# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _cas_artifact_out_of_range_expiration_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.cas_artifact(
        out.as_output(),
        # sha1 of the empty file; the digest is parsed before the timestamp so it must be valid
        "da39a3ee5e6b4b0d3255bfef95601890afd80709:0",
        "buck2-testing",
        expires_after_timestamp = 1 << 62,
    )
    return [DefaultInfo(default_output = out)]

cas_artifact_out_of_range_expiration = rule(
    impl = _cas_artifact_out_of_range_expiration_impl,
    attrs = {},
)

def _cas_artifact_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.cas_artifact(
        out.as_output(),
        ctx.attrs.digest,
        ctx.attrs.use_case,
        expires_after_timestamp = ctx.attrs.expires_after_timestamp,
        has_content_based_path = False,
    )
    return [DefaultInfo(default_output = out)]

# A file `cas_artifact` whose digest and use case come from the test.
cas_artifact = rule(
    impl = _cas_artifact_impl,
    attrs = {
        "digest": attrs.string(),
        "expires_after_timestamp": attrs.int(default = 0),
        "use_case": attrs.string(default = "buck2-testing"),
    },
)

def _write_impl(ctx):
    out = ctx.actions.write("content.txt", ctx.attrs.content, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

write = rule(
    impl = _write_impl,
    attrs = {
        "content": attrs.string(),
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
# command's use case, or fails if a CAS-backed input is not there.
consume = rule(
    impl = _consume_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)

_GENERATE = """
import hashlib
import sys

out, seed, size = sys.argv[1], sys.argv[2].encode(), int(sys.argv[3])
with open(out, "wb") as f:
    written = 0
    counter = 0
    while written < size:
        block = hashlib.sha256(seed + counter.to_bytes(8, "little")).digest()[: size - written]
        f.write(block)
        written += len(block)
        counter += 1
"""

def _generate_impl(ctx):
    out = ctx.actions.declare_output("blob", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", "-c", _GENERATE, out.as_output(), ctx.attrs.seed, str(ctx.attrs.size)]),
        category = "generate",
    )
    return [DefaultInfo(default_output = out)]

# `size` bytes derived from `seed` by chained hashing, so the same bytes come out locally and
# remotely without depending on a random number generator's implementation.
generate = rule(
    impl = _generate_impl,
    attrs = {
        "seed": attrs.string(),
        "size": attrs.int(),
    },
)
