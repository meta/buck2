# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_sources.bzl", "get_srcs_with_flags")
load("@prelude//http_archive:exec_deps.bzl", "HttpArchiveExecDeps")
load("@prelude//http_archive:unarchive.bzl", "unarchive")
load("@prelude//lean:library.bzl", "lean_default_info", "lean_library_attrs")

def _value_impl(_ctx):
    return [DefaultInfo()]

# Carries a string computed at load time, so a test can read it with `uquery --output-attribute`.
value = rule(
    impl = _value_impl,
    attrs = {"value": attrs.string()},
)

def _srcs_with_flags_impl(ctx):
    compiles = get_srcs_with_flags(ctx)
    out = ctx.actions.write("compiles.txt", [str(c.index) for c in compiles])
    return [DefaultInfo(default_output = out)]

# Lists the per-file compile index of every (source, flags) pair the cxx rules would compile.
srcs_with_flags = rule(
    impl = _srcs_with_flags_impl,
    attrs = {
        "srcs": attrs.list(attrs.one_of(attrs.source(), attrs.tuple(attrs.source(), attrs.list(attrs.arg())))),
    },
)

def _script_impl(ctx):
    out = ctx.actions.write(ctx.attrs.filename, ctx.attrs.lines, is_executable = True)
    return [DefaultInfo(default_output = out), RunInfo(args = [out])]

# An executable written by an action, so the test data needs no executable bits.
script = rule(
    impl = _script_impl,
    attrs = {
        "filename": attrs.string(),
        "lines": attrs.list(attrs.string()),
    },
)

def _two_outputs_impl(ctx):
    a = ctx.actions.write("a.txt", "a")
    b = ctx.actions.write("b.txt", "b")
    return [
        DefaultInfo(
            default_outputs = [a, b],
            sub_targets = {
                "a": [DefaultInfo(default_output = a)],
                "b": [DefaultInfo(default_output = b)],
            },
        )
    ]

two_outputs = rule(
    impl = _two_outputs_impl,
    attrs = {},
)

def _tarball_impl(ctx):
    out = ctx.actions.declare_output("pages.tar.gz")
    ctx.actions.run(
        cmd_args(
            "sh",
            "-c",
            'set -e; d="$(mktemp -d)"; mkdir -p "$d/top/pages"; for f in "[id].js" i.js d.js keep.js; do echo "$f" > "$d/top/pages/$f"; done; tar -C "$d" -czf "$(pwd)/$1" top',
            "--",
            out.as_output(),
        ),
        category = "tarball",
    )
    return [DefaultInfo(default_output = out)]

# `top/pages/{[id].js,i.js,d.js,keep.js}`, each holding its own name.
tarball = rule(
    impl = _tarball_impl,
    attrs = {},
)

def _unpack_impl(ctx):
    out, _ = unarchive(
        ctx,
        archive = ctx.attrs.archive,
        output_name = "unpacked",
        ext_type = "tar.gz",
        excludes = ctx.attrs.excludes,
        strip_prefix = "top",
        exec_deps = ctx.attrs.exec_deps[HttpArchiveExecDeps],
        prefer_local = True,
        sub_targets = [],
    )
    return [DefaultInfo(default_output = out)]

# `http_archive` without the download: unpacks a tarball built in this workspace.
unpack = rule(
    impl = _unpack_impl,
    attrs = {
        "archive": attrs.source(),
        "excludes": attrs.list(attrs.string(), default = []),
        "exec_deps": attrs.exec_dep(providers = [HttpArchiveExecDeps], default = "prelude//http_archive/tools:exec_deps"),
    },
)

def _covers_only_impl(ctx):
    return [lean_default_info(ctx, ctx.actions.write("out.txt", ""))]

# `covers` of the Lean rules on a target that builds without a Lean toolchain.
covers_only = rule(
    impl = _covers_only_impl,
    attrs = {"covers": lean_library_attrs["covers"]},
)
