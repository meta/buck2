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
load("@prelude//java/utils:java_more_utils.bzl", "get_path_separator_for_exec_os")
load("@prelude//utils:expect.bzl", "expect")
load(
    ":java_providers.bzl",
    "get_all_java_packaging_deps",
    "get_compiling_deps_tset",
)

_GWT_COMPILER_CLASS = "com.google.gwt.dev.Compiler"

def gwt_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    expect(ctx.attrs.local_workers > 0, "local workers must be greater than zero")

    output = ctx.actions.declare_output("{}.zip".format(ctx.label.name), has_content_based_path = False)

    # Write deploy files to separate directory so that the generated .zip is smaller
    deploy_output = ctx.actions.declare_output("deploy", has_content_based_path = False)

    module_deps_classpath = [dep.gwt_module for dep in get_all_java_packaging_deps(ctx, ctx.attrs.module_deps) if dep.gwt_module]
    compiling_deps_tset = get_compiling_deps_tset(ctx.actions, ctx.attrs.deps)
    deps_classpath = [dep.full_library for dep in (list(compiling_deps_tset.traverse()) if compiling_deps_tset else [])]

    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    gwt_args = cmd_args([
        java_toolchain.java[RunInfo],
        "-Dgwt.normalizeTimestamps=true",
        ctx.attrs.vm_args,
        "-classpath",
        cmd_args(module_deps_classpath + deps_classpath, delimiter = get_path_separator_for_exec_os(ctx)),
        _GWT_COMPILER_CLASS,
        "-war",
        output.as_output(),
        "-style",
        ctx.attrs.style,
        "-optimize",
        str(ctx.attrs.optimize),
        "-localWorkers",
        str(ctx.attrs.local_workers),
        "-deploy",
        deploy_output.as_output(),
    ])

    if ctx.attrs.draft_compile:
        gwt_args.add("-draftCompile")
    if ctx.attrs.strict:
        gwt_args.add("-strict")
    gwt_args.add(ctx.attrs.experimental_args)
    gwt_args.add(ctx.attrs.modules)

    ctx.actions.run(gwt_args, category = "gwt_binary")

    sub_targets = {"deploy": [DefaultInfo(default_output = deploy_output)]}

    return [
        DefaultInfo(default_output = output, sub_targets = sub_targets),
    ]

Style = ["obf", "pretty", "detailed"]

gwt_binary = prelude_rule(
    name = "gwt_binary",
    impl = gwt_binary_impl,
    docs = """
        A `gwt_binary()` rule produces a GWT (Google Web Toolkit) application
        by invoking the GWT compiler against the listed `modules`. The
        resulting output can be served as part of a web app.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "deps": attrs.list(attrs.dep(), default = []),
            "draft_compile": attrs.bool(default = False),
            "experimental_args": attrs.list(attrs.string(), default = []),
            "local_workers": attrs.int(default = 2),
            "module_deps": attrs.list(attrs.dep(), default = []),
            "modules": attrs.list(attrs.string(), default = []),
            "optimize": attrs.int(default = 9),
            "strict": attrs.bool(default = False),
            "style": attrs.enum(Style, default = "obf"),
            "vm_args": attrs.list(attrs.string(), default = []),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_java_toolchain": toolchains_common.java(),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
