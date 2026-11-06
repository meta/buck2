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
load("@prelude//utils:utils.bzl", "flatten")
load(":toolchain.bzl", "PythonBootstrapToolchainInfo")

PythonBootstrapSources = provider(fields = {"srcs": provider_field(typing.Any, default = None)})

def python_bootstrap_library_impl(ctx: AnalysisContext) -> list[Provider]:
    has_content_based_path = ctx.attrs.has_content_based_path
    tree = {src.short_path: src for src in ctx.attrs.srcs}
    output = ctx.actions.symlinked_dir("__{}__".format(ctx.attrs.name), tree, has_content_based_path = has_content_based_path)
    return [
        DefaultInfo(default_output = output),
        PythonBootstrapSources(srcs = dedupe(flatten([ctx.attrs.srcs] + [dep[PythonBootstrapSources].srcs for dep in ctx.attrs.deps]))),
    ]

def python_bootstrap_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    """
    Declares a Python binary that is intended to be used in scripts that
    bootstrap other aspects of the Buck2 prelude. Python bootstrap binaries do
    not use the Python toolchain and, as such, are highly restricted in what
    they can and can't do. In particular, bootstrap binaries can only depend on
    bootstrap libraries and can only consist of a single file.
    """
    has_content_based_path = ctx.attrs.has_content_based_path
    copy_deps = ctx.attrs.copy_deps
    run_tree_inputs = {}
    run_tree_recorded_deps = {}  # For a better error message when files collide
    for dep in ctx.attrs.deps:
        dep_srcs = dep[PythonBootstrapSources].srcs
        for src in dep_srcs:
            if src.short_path in run_tree_recorded_deps and src != run_tree_inputs[src.short_path]:
                original_dep = run_tree_recorded_deps[src.short_path]
                fail(
                    "dependency `{}` and `{}` both declare a source file named `{}`, consider renaming one of these files to avoid collision".format(
                        original_dep.label, dep.label, src.short_path
                    )
                )
            run_tree_inputs[src.short_path] = src
            run_tree_recorded_deps[src.short_path] = dep

    main = ctx.attrs.main
    if main.short_path in run_tree_inputs:
        original_dep = run_tree_recorded_deps[main.short_path].label
        fail("dependency `{}` declares a source file with the same name as main file; consider renaming it".format(original_dep))

    run_tree_inputs[main.short_path] = main
    if copy_deps:
        run_tree = ctx.actions.copied_dir("__%s__" % ctx.attrs.name, run_tree_inputs, has_content_based_path = has_content_based_path)
    else:
        run_tree = ctx.actions.symlinked_dir("__%s__" % ctx.attrs.name, run_tree_inputs, has_content_based_path = has_content_based_path)

    interpreter = ctx.attrs._python_bootstrap_toolchain[PythonBootstrapToolchainInfo].interpreter
    main_artifact = run_tree.project(main.short_path)
    run_args = cmd_args(
        [interpreter, run_tree.project(main.short_path)],
        hidden = run_tree,
    )
    return [
        DefaultInfo(default_output = main_artifact, other_outputs = [run_tree, interpreter]),
        RunInfo(args = run_args),
    ]

python_bootstrap_binary = prelude_rule(
    name = "python_bootstrap_binary",
    impl = python_bootstrap_binary_impl,
    docs = """
        A `python_bootstrap_binary()` rule declares a Python binary intended
        to be used in scripts that bootstrap other aspects of the Buck2
        prelude. Python bootstrap binaries do not use the Python toolchain
        and, as such, are highly restricted in what they can and can't do.
        In particular, bootstrap binaries can only depend on
        `python_bootstrap_library()` rules and the entry point must consist
        of a single source file.
    """,
    examples = None,
    further = None,
    attrs = {
        "copy_deps": attrs.bool(default = True),
        "deps": attrs.list(attrs.dep(providers = [PythonBootstrapSources]), default = []),
        "has_content_based_path": attrs.bool(default = False),
        "main": attrs.source(),
        "_python_bootstrap_toolchain": toolchains_common.python_bootstrap(),
    }
    | buck.labels_arg()
    | buck.contacts_arg(),
)

python_bootstrap_library = prelude_rule(
    name = "python_bootstrap_library",
    impl = python_bootstrap_library_impl,
    docs = """
        A `python_bootstrap_library()` rule groups together Python sources
        that may be consumed by `python_bootstrap_binary()` targets or by
        other `python_bootstrap_library()` rules. Bootstrap libraries do
        not use the Python toolchain and may only be depended on by other
        bootstrap libraries or bootstrap binaries.
    """,
    examples = None,
    further = None,
    attrs = {
        "deps": attrs.list(attrs.dep(providers = [PythonBootstrapSources]), default = []),
        "has_content_based_path": attrs.bool(default = False),
        "srcs": attrs.list(attrs.source()),
    }
    | buck.labels_arg()
    | buck.contacts_arg(),
)
