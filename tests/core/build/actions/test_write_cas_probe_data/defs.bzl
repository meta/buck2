# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write_impl(ctx):
    out = ctx.actions.write("out.txt", ctx.attrs.content, has_content_based_path = False)
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

# Reads `dep`'s output in an action, which uploads that output to the CAS when the action runs
# remotely.
consume = rule(
    impl = _consume_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)

def _copy_source_impl(ctx):
    out = ctx.actions.copy_file("copied.txt", ctx.attrs.src)
    return [DefaultInfo(default_output = out)]

# Copies a source file in the daemon, as the copy action does, rather than through a command.
copy_source = rule(
    impl = _copy_source_impl,
    attrs = {
        "src": attrs.source(),
    },
)

def _copied_dir_sources_impl(ctx):
    out = ctx.actions.copied_dir("copied", {src.basename: src for src in ctx.attrs.srcs})
    return [DefaultInfo(default_output = out)]

# Copies source files into a directory in the daemon, as `copied_dir` does.
copied_dir_sources = rule(
    impl = _copied_dir_sources_impl,
    attrs = {
        "srcs": attrs.list(attrs.source()),
    },
)

def _consume_source_impl(ctx):
    out = ctx.actions.declare_output("copy.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["cp", ctx.attrs.src, out.as_output()]),
        category = "consume",
    )
    return [DefaultInfo(default_output = out)]

# Reads a source file in an action, which uploads it to the CAS when the action runs remotely.
consume_source = rule(
    impl = _consume_source_impl,
    attrs = {
        "src": attrs.source(),
    },
)
