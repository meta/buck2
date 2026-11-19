# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
A bundled runtime includes the full install of python in the par, allowing a fully hermetic distribution.
This rule defines the artifacts which may be include.
include - the headers files required for compiling against the runtime
libpython - the shared object `libpython.so` which has all of the required symbols for the runtime
py_bin - the python executable
py_version - a string denoting the version of python represented by this provider
stdlib - the path to the python standard library
"""

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")

PythonRuntimeBundleInfo = provider(
    fields = {
        "include": provider_field(Artifact),
        "libpython": provider_field(Artifact | None),
        "py_bin": provider_field(Artifact),
        "py_version": provider_field(str),
        "shared_libs": provider_field(list[Dependency]),
        "stdlib": provider_field(Artifact),
    }
)

def python_runtime_bundle_impl(ctx: AnalysisContext) -> list[Provider]:
    root = ctx.attrs.install_root[DefaultInfo].default_outputs[0]
    info = PythonRuntimeBundleInfo(
        py_version = ctx.attrs.py_version,
        py_bin = root.project(ctx.attrs.py_bin),
        stdlib = root.project(ctx.attrs.stdlib),
        libpython = root.project(ctx.attrs.libpython) if ctx.attrs.libpython else None,
        include = root.project(ctx.attrs.include),
        shared_libs = ctx.attrs.shared_libs,
    )
    return [DefaultInfo(default_output = root), info]

python_runtime_bundle = prelude_rule(
    name = "python_runtime_bundle",
    impl = python_runtime_bundle_impl,
    docs = """
        A `python_runtime_bundle()` rule declares a bundled Python runtime
        — a full install of Python including the interpreter, standard
        library, headers, and optionally `libpython.so` and additional
        shared libraries — that can be embedded in a PAR to produce a
        fully hermetic distribution.
    """,
    examples = None,
    further = None,
    attrs = {
        "include": attrs.string(doc = "Header files required for linking python extensions"),
        "install_root": attrs.dep(doc = "The filegroup containing the runtime artifacts, all the paths are relative to this location"),
        "libpython": attrs.option(
            attrs.string(doc = "libpyhon.so required at runtime for the python executable and native extensions."),
            default = None,
        ),
        "py_bin": attrs.string(doc = "The runtime executable"),
        "py_version": attrs.string(doc = "The version of python this represents"),
        "shared_libs": attrs.list(attrs.dep(), default = [], doc = "Additional shared libraries required by this runtime"),
        "stdlib": attrs.string(doc = "The python standard library"),
    }
    | buck.labels_arg()
    | buck.contacts_arg(),
)
