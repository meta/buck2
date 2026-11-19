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
load("@prelude//haskell:compile.bzl", "compile_args")
load("@prelude//haskell:link_info.bzl", "cxx_toolchain_link_style")
load(
    "@prelude//haskell:toolchain.bzl",
    "HaskellToolchainInfo",
)
load(
    "@prelude//haskell:util.bzl",
    "attr_deps",
)
load("@prelude//utils:argfile.bzl", "at_argfile")

HaskellHaddockInfo = provider(
    fields = {
        "html": provider_field(typing.Any, default = None),
        "interface": provider_field(typing.Any, default = None),
    },
)

def haskell_haddock_lib(ctx: AnalysisContext, pkgname: str) -> Provider:
    haskell_toolchain = ctx.attrs._haskell_toolchain[HaskellToolchainInfo]

    iface = ctx.actions.declare_output("haddock-interface", has_content_based_path = False)
    odir = ctx.actions.declare_output("haddock-html", dir = True, has_content_based_path = False)

    link_style = cxx_toolchain_link_style(ctx)
    args = compile_args(
        ctx,
        link_style,
        enable_profiling = False,
        suffix = "-haddock",
        pkgname = pkgname,
    )

    cmd = cmd_args(haskell_toolchain.haddock)
    cmd.add(cmd_args(args.args_for_cmd, format = "--optghc={}"))
    cmd.add(
        "--use-index",
        "doc-index.html",
        "--use-contents",
        "index.html",
        "--html",
        "--hoogle",
        "--no-tmp-comp-dir",
        "--no-warnings",
        "--dump-interface",
        iface.as_output(),
        "--odir",
        odir.as_output(),
        "--package-name",
        pkgname,
    )

    for lib in attr_deps(ctx):
        hi = lib.get(HaskellHaddockInfo)
        if hi != None:
            cmd.add("--read-interface", hi.interface)

    cmd.add(ctx.attrs.haddock_flags)

    source_entity = read_root_config("haskell", "haddock_source_entity", None)
    if source_entity:
        cmd.add("--source-entity", source_entity)

    if args.args_for_file:
        if haskell_toolchain.use_argsfile:
            ghcargs = cmd_args(args.args_for_file, format = "--optghc={}")
            cmd.add(
                at_argfile(
                    actions = ctx.actions,
                    name = "args.haskell_haddock_argsfile",
                    args = [ghcargs, args.srcs],
                    allow_args = True,
                )
            )
        else:
            cmd.add(args.args_for_file)

    # Buck2 requires that the output artifacts are always produced, but Haddock only
    # creates them if it needs to, so we need a wrapper script to mkdir the outputs.
    script = ctx.actions.declare_output("haddock-script", has_content_based_path = False)
    script_args = cmd_args(
        [
            "mkdir",
            "-p",
            args.result.objects.as_output(),
            args.result.hi.as_output(),
            args.result.stubs.as_output(),
            "&&",
            cmd_args(cmd, quote = "shell"),
        ],
        delimiter = " ",
    )
    ctx.actions.write(
        script,
        cmd_args("#!/bin/sh", script_args),
        is_executable = True,
        allow_args = True,
    )

    ctx.actions.run(
        cmd_args(script, hidden = cmd),
        category = "haskell_haddock",
        no_outputs_cleanup = True,
    )

    return HaskellHaddockInfo(interface = iface, html = odir)

def haskell_haddock_impl(ctx: AnalysisContext) -> list[Provider]:
    haskell_toolchain = ctx.attrs._haskell_toolchain[HaskellToolchainInfo]

    out = ctx.actions.declare_output("haddock-html", dir = True, has_content_based_path = False)

    cmd = cmd_args(haskell_toolchain.haddock)

    cmd.add(
        "--gen-index",
        "--gen-contents",
        "-o",
        out.as_output(),
    )

    dep_htmls = []
    for lib in attr_deps(ctx):
        hi = lib.get(HaskellHaddockInfo)
        if hi != None:
            cmd.add("--read-interface", hi.interface)
            dep_htmls.append(hi.html)

    cmd.add(ctx.attrs.haddock_flags)

    script = ctx.actions.declare_output("haddock-script", has_content_based_path = False)
    script_args = cmd_args([
        "#!/bin/sh",
        "set -ueo pipefail",
        cmd_args(cmd, delimiter = " ", quote = "shell"),
    ])
    for dir in dep_htmls:
        script_args.add(
            cmd_args(
                ["cp", "-Rf", "--reflink=auto", cmd_args(dir, format = "{}/*"), out.as_output()],
                delimiter = " ",
            ),
        )
    ctx.actions.write(
        script,
        script_args,
        is_executable = True,
        allow_args = True,
    )

    ctx.actions.run(
        cmd_args(script, hidden = script_args),
        category = "haskell_haddock",
        no_outputs_cleanup = True,
    )

    return [DefaultInfo(default_outputs = [out])]

haskell_haddock = prelude_rule(
    name = "haskell_haddock",
    impl = haskell_haddock_impl,
    docs = """
        A `haskell_haddock()` rule generates Haddock documentation for
        the given Haskell dependencies. Extra flags can be passed to the
        `haddock` tool via `haddock_flags`.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "deps": attrs.list(attrs.dep(), default = []),
            "deps_query": attrs.option(attrs.query(), default = None),
            "haddock_flags": attrs.list(attrs.arg(), default = []),
            "platform": attrs.option(attrs.string(), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "_cxx_toolchain": toolchains_common.cxx(),
            "_haskell_toolchain": toolchains_common.haskell(),
        }
    ),
)
