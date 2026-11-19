# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _result(ctx) -> dict:
    data = {
        "message": "Here I am describing the failure reason" if ctx.attrs.fail else None,
        # `passed` is not a valid status: a malformed result file.
        "status": "passed" if ctx.attrs.malformed else "failure" if ctx.attrs.fail else "success",
    }
    if ctx.attrs.shape == "no_version":
        return {"data": data}
    if ctx.attrs.shape == "other_version":
        # Written for another version of the format: a different layout.
        return {"result": data, "version": 2}
    return {"data": data, "version": 1}

def _impl(ctx) -> list[Provider]:
    flute = ctx.actions.write_json(
        "flute.json",
        _result(ctx),
        pretty = True,
        has_content_based_path = False,
    )
    return [
        DefaultInfo(),
        RunInfo(args = ["echo", "hello"]),
        ExternalRunnerTestInfo(type = "dummy", command = ["true"]),
        InstallInfo(installer = ctx.attrs.installer, files = {"random_file": flute}),
        ValidationInfo(
            validations = [
                ValidationSpec(
                    name = "whistle",
                    validation_result = flute,
                    optional = ctx.attrs.optional,
                ),
            ],
        ),
    ]

china = rule(
    impl = _impl,
    attrs = {
        "dep": attrs.option(attrs.dep(), default = None),
        "fail": attrs.bool(default = False),
        "installer": attrs.default_only(attrs.label(default = "//:my_installer")),
        "malformed": attrs.bool(default = False),
        "optional": attrs.bool(default = False),
        "shape": attrs.enum(["current", "no_version", "other_version"], default = "current"),
    },
)

def _installer_impl(ctx) -> list[Provider]:
    return [
        DefaultInfo(),
        RunInfo(args = ["fbpython", ctx.attrs.main]),
    ]

installer = rule(
    impl = _installer_impl,
    attrs = {
        "main": attrs.source(),
    },
)
