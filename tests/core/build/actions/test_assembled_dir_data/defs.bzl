# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _mixed_impl(ctx):
    exe = ctx.actions.write("exe.src", "exe-bytes")
    manifest = ctx.actions.write("manifest.src", "manifest-bytes")
    res = ctx.actions.copied_dir("res.src", {"data.txt": ctx.actions.write("data.txt", "resource-bytes")})

    out = ctx.actions.assembled_dir(
        "out",
        contents = {
            "bin/exe": assembled_dir.copy(exe),
            "bin/exe.resources.json": assembled_dir.copy(manifest),
            "deep/nested/link": assembled_dir.symlink(exe),
            "res": assembled_dir.symlink(res),
            "src_copy.txt": assembled_dir.copy(ctx.attrs.src),
            "src_link.txt": assembled_dir.symlink(ctx.attrs.src),
        },
    )

    return [DefaultInfo(default_output = out)]

mixed = rule(
    impl = _mixed_impl,
    attrs = {
        "src": attrs.source(),
    },
)

def _consumed_impl(ctx):
    dir = ctx.attrs.dep[DefaultInfo].default_outputs[0]

    script = ctx.actions.write(
        "cat.py",
        [
            "import sys",
            "from pathlib import Path",
            "d = Path(sys.argv[1])",
            "copied = (d / 'bin' / 'exe').read_text()",
            "linked = (d / 'src_link.txt').read_text()",
            "Path(sys.argv[2]).write_text(copied + '|' + linked)",
        ],
    )

    out = ctx.actions.declare_output("out.txt")
    ctx.actions.run(
        cmd_args(["fbpython", script, dir, out.as_output()]),
        category = "consume",
    )

    return [DefaultInfo(default_output = out)]

consumed = rule(
    impl = _consumed_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)

def _overlap_fail_impl(ctx):
    f = ctx.actions.write("f.src", "f")
    out = ctx.actions.assembled_dir(
        "out",
        contents = {
            "a": assembled_dir.copy(f),
            "a/b": assembled_dir.symlink(f),
        },
    )
    return [DefaultInfo(default_output = out)]

overlap_fail = rule(
    impl = _overlap_fail_impl,
    attrs = {},
)

def _empty_path_fail_impl(ctx):
    f = ctx.actions.write("f.src", "f")
    out = ctx.actions.assembled_dir(
        "out",
        contents = {
            "": assembled_dir.copy(f),
        },
    )
    return [DefaultInfo(default_output = out)]

empty_path_fail = rule(
    impl = _empty_path_fail_impl,
    attrs = {},
)

def _untyped_entry_fail_impl(ctx):
    f = ctx.actions.write("f.src", "f")
    out = ctx.actions.assembled_dir(
        "out",
        contents = {
            "f": f,
        },
    )
    return [DefaultInfo(default_output = out)]

untyped_entry_fail = rule(
    impl = _untyped_entry_fail_impl,
    attrs = {},
)
