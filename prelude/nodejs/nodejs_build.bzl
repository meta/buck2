# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(":nodejs_binary.bzl", "stage_nodejs_entries")
load(":nodejs_providers.bzl", "check_relative_path")
load(":nodejs_toolchain.bzl", "NodejsToolchainInfo")

# Replaced by the project root when the build runs, from another directory.
_ROOT_PREFIX = "__NODEJS_BUILD_ROOT__/"

def nodejs_build_impl(ctx: AnalysisContext) -> list[Provider]:
    node_exec = ctx.attrs._nodejs_toolchain[NodejsToolchainInfo].node_exec
    if node_exec == None:
        fail("{}: nodejs_build requires `node_exec` on the nodejs toolchain".format(ctx.label))
    if check_relative_path(ctx.attrs.out, "{}: invalid `out`".format(ctx.label))[0] == "node_modules":
        fail("{}: `out` '{}' cannot be under `node_modules`".format(ctx.label, ctx.attrs.out))
    runs = [attr for attr in ("main", "library", "tool") if getattr(ctx.attrs, attr) != None]
    if len(runs) != 1:
        fail(
            "{}: exactly one of `main`, `library` and `tool` is required, got {}".format(ctx.label, ", ".join(["`{}`".format(attr) for attr in runs]) or "none")
        )
    if ctx.attrs.tool != None:
        check_relative_path(ctx.attrs.tool, "{}: invalid `tool`".format(ctx.label))
        staged = stage_nodejs_entries(ctx, ctx.attrs.srcs, mains_attr = "`srcs`", staged_attrs = "`srcs`", srcs = [], stage = True)
        entry = "node_modules/" + ctx.attrs.tool
    else:
        staged = stage_nodejs_entries(ctx, [ctx.attrs.main] if ctx.attrs.main != None else [], stage = True)
        entry = staged.dir_paths[0]

    out = ctx.actions.declare_output("out", dir = True)
    cmd = cmd_args(
        ctx.attrs._package_tree_tool[RunInfo],
        "build",
        "--dir",
        staged.dir,
        "--output-path",
        ctx.attrs.out,
        "--out",
        out.as_output(),
        "--root-prefix",
        _ROOT_PREFIX,
    )
    for key in ctx.attrs.env:
        cmd.add("--env-key", key)
    cmd.add("--command", cmd_args(node_exec, ctx.attrs.node_args, entry, ctx.attrs.args, absolute_prefix = _ROOT_PREFIX))
    ctx.actions.run(
        cmd,
        env = {key: cmd_args(value, absolute_prefix = _ROOT_PREFIX) for key, value in ctx.attrs.env.items()},
        category = "nodejs_build",
    )
    return [DefaultInfo(default_output = out)]

nodejs_build = prelude_rule(
    name = "nodejs_build",
    impl = nodejs_build_impl,
    docs = """Runs a Node.js build tool, such as a bundler, as a build
    action, and outputs the directory it writes. The tool runs with the
    nodejs toolchain's `node_exec` in a writable copy of the directory
    `nodejs_binary` stages as `[dir]`: `srcs`, `main`, `resources` and
    data deps at their paths, and a `node_modules` composed from `deps`,
    `library` and `node_modules` as for `nodejs_binary`. Files outside
    `node_modules` are copied; each entry of `node_modules` is a symlink
    to its staged copy, so a tool that resolves symlinks, as Node and
    Vite do by default, finds packages there rather than in the copy, and
    writes under `node_modules` (such as a tool's cache) stay in the copy.

    Exactly one of these runs, with `node_args` before it and `args`
    after it: `main`, a file that is staged; `library` with
    `entry_point`, as for `nodejs_binary`; or `tool`, the path of a file
    under the composed `node_modules`, such as `"vite/bin/vite.js"` for a
    third-party tool, which then needs `srcs`. The working directory is
    the copy's root, so relative paths in `args` and in the tool's config
    name staged files. `$(location)` macros in `args` and `env` expand to
    absolute paths.

    `out` is the relative, `/`-separated path of the directory the tool
    writes, such as `"dist"` or `"out/renderer"`; it cannot be under
    `node_modules`. That directory, with symlinks copied as the files they
    resolve to, is the default output. The build fails if the tool exits
    non-zero or writes no directory at `out`. A tool that records the
    paths of the packages it reads, as source maps do, records their
    staged location.

    The output is a built artifact, like a binary, not a package: a
    bundler's output cannot be composed further as a dependency, so this
    rule belongs at or near the top of a build graph. It provides no
    `NodejsLibraryInfo`, so it never joins another target's
    `node_modules`. Consume it as a resource: in `resources`, or as a
    `deps` entry, which stages it as a data dep at its package path.""",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "main": attrs.option(attrs.source(), default = None),
            "tool": attrs.option(attrs.string(), default = None),
            "out": attrs.string(),
            "srcs": attrs.list(attrs.source(), default = []),
            "resources": attrs.named_set(attrs.source(allow_directory = True), sorted = False, default = []),
            "args": attrs.list(attrs.arg(), default = []),
            "node_args": attrs.list(attrs.string(), default = []),
            "env": attrs.dict(attrs.string(), attrs.arg(), default = {}),
            "deps": attrs.list(attrs.dep(), default = []),
            "library": attrs.option(attrs.dep(), default = None),
            "entry_point": attrs.option(attrs.string(), default = None),
            "node_modules": attrs.option(attrs.list(attrs.dep()), default = None),
            "_nodejs_toolchain": toolchains_common.nodejs(),
            "_package_tree_tool": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//nodejs/tools:package_tree")),
        }
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
