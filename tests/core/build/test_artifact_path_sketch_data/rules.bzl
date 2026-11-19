# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _simple_write_impl(ctx):
    out = ctx.actions.write(ctx.attrs.out, "content")
    return [DefaultInfo(default_outputs = [out])]

simple_write = rule(
    impl = _simple_write_impl,
    attrs = {
        "out": attrs.string(default = "out.txt"),
    },
)

def _copy_dep_impl(ctx):
    out = ctx.actions.copy_file("out.txt", ctx.attrs.dep[DefaultInfo].default_outputs[0])
    return [DefaultInfo(default_outputs = [out])]

copy_dep = rule(
    impl = _copy_dep_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)

def _content_based_write_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = True)
    ctx.actions.write(out, "content")
    return [DefaultInfo(default_outputs = [out])]

content_based_write = rule(impl = _content_based_write_impl, attrs = {})

def _projected_output_impl(ctx):
    out_dir = ctx.actions.declare_output("out_dir", dir = True)
    ctx.actions.run(
        cmd_args(
            "sh",
            "-c",
            'mkdir -p "$1" && echo a > "$1/a"',
            "--",
            out_dir.as_output(),
        ),
        category = "create_dir",
    )
    return [DefaultInfo(default_outputs = [out_dir.project("a")])]

projected_output = rule(impl = _projected_output_impl, attrs = {})

def _symlink_rule_impl(ctx):
    src = ctx.actions.write("src.txt", "source_content")
    out = ctx.actions.symlink_file("link.txt", src)
    return [DefaultInfo(default_outputs = [out])]

symlink_rule = rule(impl = _symlink_rule_impl, attrs = {})

def _symlinked_dir_rule_impl(ctx):
    src1 = ctx.actions.write("src1.txt", "content1")
    src2 = ctx.actions.write("src2.txt", "content2")
    out = ctx.actions.symlinked_dir("out_dir", {"file1": src1, "file2": src2})
    return [DefaultInfo(default_outputs = [out])]

symlinked_dir_rule = rule(impl = _symlinked_dir_rule_impl, attrs = {})
