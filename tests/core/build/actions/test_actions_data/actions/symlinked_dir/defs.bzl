# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def write_file_impl(ctx):
    out = ctx.actions.write(ctx.attrs.out, ctx.attrs.contents)
    return [DefaultInfo(default_output = out)]

def symlink_files_impl(ctx):
    srcs = {src.short_path: src for src in ctx.attrs.srcs}

    # Also make sure that linking to a new location works properly
    srcs.update({"subdir/{}.suffix".format(src.short_path): src for src in ctx.attrs.srcs})
    out = ctx.actions.symlinked_dir("out", srcs, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

write_file = rule(
    impl = write_file_impl,
    attrs = {
        "contents": attrs.string(),
        "out": attrs.string(),
    },
)

symlink_files = rule(
    impl = symlink_files_impl,
    attrs = {
        "srcs": attrs.list(attrs.source()),
    },
)

def write_transitive_file_impl(ctx):
    # Set up a transitive artifact to demonstrate that they aren't handled correctly
    # TODO(T227006457) - fix this quirk and invert the corresponding test case
    transitive_1 = ctx.actions.write("tdep1", "transitive content", has_content_based_path = False)
    out = ctx.actions.write("out_file", "out content", has_content_based_path = False).with_associated_artifacts([transitive_1])
    return [DefaultInfo(default_output = out)]

def symlink_transitive_files_impl(ctx):
    srcs = {src.short_path: src for src in ctx.attrs.srcs}
    out = ctx.actions.symlinked_dir("out_dir", srcs, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

write_transitive_file = rule(
    impl = write_transitive_file_impl,
    attrs = {},
)

symlink_transitive_files = rule(
    impl = symlink_transitive_files_impl,
    attrs = {
        "srcs": attrs.list(attrs.source()),
    },
)
