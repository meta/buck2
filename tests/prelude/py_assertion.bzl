# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx: AnalysisContext) -> list[Provider]:
    marker = ctx.actions.declare_output("marker", has_content_based_path = False)
    write_args = cmd_args(
        ctx.attrs.script,
        cmd_args(
            "from pathlib import Path; ",
            "import sys; ",
            'open(Path(sys.argv[0]).parent / "marker", "w").write("")',
            delimiter = "",
        ),
        hidden = marker.as_output(),
    )
    script, _ = ctx.actions.write("script.py", write_args, allow_args = True, has_content_based_path = False)
    ctx.actions.run(
        cmd_args("fbpython", script, ctx.attrs.script_args, hidden = write_args),
        category = "py_assertion",
    )
    return [
        DefaultInfo(default_output = marker),
    ]

_py_assertion = rule(
    impl = _impl,
    attrs = {
        "script": attrs.arg(),
        "script_args": attrs.list(attrs.arg(), default = []),
    },
)

def py_assertion(name: str, exec_compatible_with: list[str] = [], **kwargs):
    exec_compatible_with = list(exec_compatible_with)

    # Don't allow cross-running these tests, since they often invoke executables built for the
    # target platform
    exec_compatible_with.append(
        select({
            "ovr_config//os:linux": "ovr_config//os:linux",
            "ovr_config//os:macos": "ovr_config//os:macos",
            "ovr_config//os:windows": "ovr_config//os:windows",
        })
    )
    exec_compatible_with.append(
        select({
            "ovr_config//cpu:arm64": "ovr_config//cpu:arm64",
            "ovr_config//cpu:x86_64": "ovr_config//cpu:x86_64",
        })
    )

    _py_assertion(name = name, exec_compatible_with = exec_compatible_with, **kwargs)
