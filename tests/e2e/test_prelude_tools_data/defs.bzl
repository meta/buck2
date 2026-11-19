# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _manifest_module_impl(ctx):
    manifest = ctx.actions.write_json(
        "module_manifest.json",
        [[path, path, "root//:manifest_module"] for path in ctx.attrs.paths],
    )
    manifests = ctx.actions.write("module_manifests.txt", [manifest])
    out = ctx.actions.declare_output("__manifest__.py")
    ctx.actions.run(
        cmd_args(
            ctx.attrs._tool[RunInfo],
            "--module-manifests",
            manifests,
            "--output",
            out.as_output(),
            hidden = [manifest],
        ),
        category = "manifest_module",
    )
    return [DefaultInfo(default_output = out)]

# `__manifest__.py` for a module manifest, generated as make_py_package.bzl generates it.
manifest_module = rule(
    impl = _manifest_module_impl,
    attrs = {
        "paths": attrs.list(attrs.string()),
        "_tool": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//python/tools:make_py_package_manifest_module")),
    },
)
