# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _prebuilt_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    exe = ctx.attrs.exe
    args = ctx.attrs.extra_args
    return [
        DefaultInfo(default_output = exe),
        RunInfo(args = [exe] + args),
    ]

prebuilt_binary = rule(
    impl = _prebuilt_binary_impl,
    attrs = {
        "exe": attrs.source(),
        "extra_args": attrs.list(attrs.string(), default = []),
    },
)

def _installer_impl(ctx: AnalysisContext) -> list[Provider]:
    installer = ctx.attrs.installer
    return [
        DefaultInfo(),
        InstallInfo(
            installer = installer,
            files = ctx.attrs.files,
        ),
    ]

installer = rule(
    impl = _installer_impl,
    attrs = {
        "files": attrs.dict(key = attrs.string(), value = attrs.source(), default = {}),
        "installer": attrs.label(),
    },
)

def _export_file_impl(ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(default_output = ctx.attrs.src)]

export_file = rule(
    impl = _export_file_impl,
    attrs = {
        "src": attrs.source(),
    },
)

def _fail_build_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(cmd_args("false", hidden = out.as_output()), category = "fail_build", local_only = True)
    return [
        DefaultInfo(default_output = out),
        RunInfo(args = [out]),
    ]

fail_build = rule(impl = _fail_build_impl, attrs = {})
