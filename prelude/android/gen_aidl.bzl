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
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load(":android_toolchain.bzl", "AidlToolchainInfo")

_AidlSourceInfo = provider(
    fields = {
        "srcs": provider_field(typing.Any, default = None),
    }
)

def gen_aidl_impl(ctx: AnalysisContext) -> list[Provider]:
    aidl_toolchain = ctx.attrs._aidl_toolchain[AidlToolchainInfo]
    aidl_cmd = cmd_args(
        [aidl_toolchain.aidl]
        + ["-p", aidl_toolchain.framework_aidl_file]
        + ["-I", ctx.attrs.import_path]
        + [a for path in ctx.attrs.import_paths for a in ["-I", path]],
        # We need the `aidl_srcs` files - otherwise the search on the `import_path` won't find anything.
        hidden = ctx.attrs.aidl_srcs,
    )

    # Allow gen_aidl rules to depend on other gen_aidl rules, and make the source files from the
    # deps accessible in this context. This is an alternative to adding dependent files in
    # aidl_srcs.
    dep_srcs = []
    for dep in ctx.attrs.deps:
        source_info = dep.get(_AidlSourceInfo)
        if source_info != None:
            dep_srcs += source_info.srcs
        else:
            deprecation(
                "starlark_gen_aidl_non_aidl_dep",
                "`{}` dependency `{}` is not a `gen_aidl` rule and will be ignored".format(ctx.label, dep.label),
                owner = "jwm",
                started = "2022-10-17",
            )

    aidl_cmd.add(cmd_args(hidden = dep_srcs))

    aidl_out = ctx.actions.declare_output("aidl_output", dir = True, has_content_based_path = True)
    aidl_cmd.add("-o", aidl_out.as_output())
    aidl_cmd.add(ctx.attrs.aidl)

    # Aidl does not create any output for parcelables. Therefore, we always initialize the output
    # directory so that we don't get an "Action failed to produce outputs" error.
    if ctx.attrs._exec_os_type[OsLookup].os == Os("windows"):
        sh_cmd = cmd_args([
            cmd_args(["cmd.exe", "/c", cmd_args([aidl_out.as_output()], format = "if not exist {} md {}")]),
            "&&",
            aidl_cmd,
        ])
    else:
        sh_cmd = cmd_args([
            "sh",
            "-c",
            "mkdir -p $1 && $2",
            "--",
            aidl_out.as_output(),
            cmd_args(aidl_cmd, delimiter = " "),
        ])

    ctx.actions.run(sh_cmd, category = "aidl")

    # Put the generated Java files into a zip file to be used as srcs to other rules.
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    jar_cmd = cmd_args(java_toolchain.jar)
    jar_cmd.add("-cfM")
    out = ctx.actions.declare_output("{}_aidl_java_output.src.zip".format(ctx.attrs.name), has_content_based_path = True)
    jar_cmd.add(out.as_output())
    jar_cmd.add(aidl_out)

    ctx.actions.run(jar_cmd, category = "aidl_jar")

    return [
        DefaultInfo(default_output = out),
        _AidlSourceInfo(srcs = [ctx.attrs.aidl] + ctx.attrs.aidl_srcs + dep_srcs),
    ]

gen_aidl = prelude_rule(
    name = "gen_aidl",
    impl = gen_aidl_impl,
    docs = """
        A `gen_aidl()` rule is used to generate `.java` files
        from `.aidl` files.
    """,
    examples = """
        ```
        android_library(
          name = 'lib',
          srcs = glob(['**/*.java']) + [':aidl'],
          manifest = '//res/org/opencv:manifest',
          deps = [
            '//res/org/opencv:res',
          ],
          visibility = [ 'PUBLIC' ],
        )

        gen_aidl(
            name = 'aidl',
            aidl = 'engine/OpenCVEngineInterface.aidl',
            import_path = 'java/',
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "aidl": attrs.source(
                doc = """
                The path to an `.aidl` file to convert to a
                 `.java` file.
            """,
            ),
            "import_path": attrs.string(
                default = "",
                doc = """
                The search path for import statements for the aidl command.
                 (This is the `-I` argument when invoking aidl
                 from the command line. For many apps it will be the base dir where all
                 aidl files are, with project root as its parent,
                 e.g. `app/src/main/aidl`.). This is the same as the path to the
                 `aidl` file relative to what would be returned from `root`.
            """,
            ),
            "import_paths": attrs.list(
                attrs.arg(),
                default = [],
                doc = """
                A list of additional import statements for the aidl command.
                 (This appends an `-I` argument for each of the provided paths when
                 invoking aidl from the command line.
            """,
            ),
            "aidl_srcs": attrs.set(
                attrs.source(),
                sorted = True,
                default = [],
                doc = """
                Path to `.aidl` files the target `aidl` file imports.
            """,
            ),
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                A list of rules that must be built before this rule.
            """,
            ),
            "_aidl_toolchain": toolchains_common.android_aidl(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_java_toolchain": toolchains_common.java_for_android(),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
