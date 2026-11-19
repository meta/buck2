# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:command_alias.bzl", "command_alias")
load("@prelude//:platform_exe.bzl", "resolve_platform_exe")
load("@prelude//:platform_exe_constants.bzl", "SUPPORTED_ARCHS", "SUPPORTED_OSES")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load("@prelude//python:manifest.bzl", "create_manifest_for_entries")
load("@prelude//unix:providers.bzl", "UnixEnv", "create_unix_env_info")

def _export_exe_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs.src and ctx.attrs.exe:
        fail("export_exe accepts at most one of src or exe")

    src = ctx.attrs.src if ctx.attrs.src else ctx.attrs.exe
    if ctx.attrs.platform_exe != None and len(ctx.attrs.platform_exe) == 0:
        fail("export_exe platform_exe must not be empty")
    if ctx.attrs.platform_exe != None:
        return _platform_export_exe(ctx, src)

    providers = []
    providers.append(DefaultInfo(other_outputs = ctx.attrs.resources))
    providers.append(RunInfo(args = cmd_args(src, hidden = ctx.attrs.resources)))

    if ctx.attrs.src != None:
        providers.append(
            create_unix_env_info(
                actions = ctx.actions,
                env = UnixEnv(
                    label = ctx.label,
                    binaries = [
                        create_manifest_for_entries(
                            ctx = ctx,
                            name = "unix_env",
                            entries = [
                                (ctx.attrs.src.basename, ctx.attrs.src, ""),
                            ],
                        ),
                    ],
                ),
            ),
        )

    return providers

_export_exe = rule(
    doc = """Exports a file as an executable, for use in $(exe) macros or as a valid target for an exec_dep().
    Accepts either a string `src`, which is a relative path to a file that will be directly referenced,
    or an arg `exe` which should be a path to an executable relative to a $(location) macro.

    The first form is a more ergonomic replacement for export_file + command_alias. Eg. Instead of

    export_file(
        name = "script_sh",
        src = "bin/script.sh",
    )

    command_alias(
        name = "script.sh"
        exe = ":script_sh",
    )

    You can write

    export_exe(
        name = "script.sh",
        src = "bin/script.sh",
    )

    The latter form allows executing checked in binaries with required resources (eg. runtime shared libraries)
    without unnecessary indirection via another rule which allows args, like command_alias. Eg. instead of

    export_file(
        name = "bin"
        src = "bin",
        mode = "reference",
    )

    export_file(
        name = "exec.sh",
        src = "exec.sh",
    )

    command_alias(
        name = "compiler",
        exe = ":exec.sh", # Just calls exec $@
        args = [
            "$(location :bin)/compiler",
        ],
    )

    You can write

    export_file(
        name = "bin",
        src = "bin",
        mode = "reference",
    )

    export_exe(
        name = "compiler",
        exe = "$(location :bin)/compiler",
    )

    To ensure additional resources are materialized in Remote Execution (e.g., internal
    compiler components that the executable invokes), use the `resources` parameter:

    export_file(
        name = "bin",
        src = "bin",
        mode = "reference",
    )

    export_file(
        name = "libexec",
        src = "libexec",
        mode = "reference",
    )

    export_exe(
        name = "compiler",
        exe = "$(location :bin)/compiler",
        resources = [":bin", ":libexec"],
    )
    """,
    impl = _export_exe_impl,
    attrs = {
        "exe": attrs.option(attrs.arg(), default = None, doc = "arg which should evaluate to a path to an executable binary"),
        "platform_exe": attrs.option(
            attrs.dict(
                key = attrs.enum(SUPPORTED_OSES),
                value = attrs.one_of(attrs.dep(), attrs.dict(key = attrs.enum(SUPPORTED_ARCHS), value = attrs.dep())),
                sorted = False,
            ),
            default = None,
        ),
        "resources": attrs.list(attrs.source(), default = [], doc = "Additional artifacts to materialize alongside the executable (for Remote Execution)"),
        "src": attrs.option(attrs.source(), default = None, doc = "path to an executable binary relative to this package"),
        "_target_os_type": buck.target_os_type_arg(),
    },
)

def _platform_export_exe(ctx: AnalysisContext, fallback) -> list[Provider]:
    base = resolve_platform_exe(
        RunInfo(args = cmd_args(fallback)) if fallback != None else None,
        ctx.attrs.platform_exe,
        ctx.attrs._target_os_type[OsLookup],
    )
    output = command_alias(
        actions = ctx.actions,
        path = "__export_exe_trampoline",
        target_os = ctx.attrs._target_os_type[OsLookup],
        base = base,
        args = cmd_args(),
        env = {},
        labels = [],
    )
    return [
        DefaultInfo(default_outputs = output.output.default_outputs, other_outputs = output.output.other_outputs + ctx.attrs.resources),
        RunInfo(args = cmd_args(output.cmd, hidden = ctx.attrs.resources)),
    ]

def export_exe(name, exe = None, src = None, platform_exe = None, **kwargs):
    """Export `src`, `exe`, or a `command_alias.platform_exe` mapping."""
    # If no executable or platform map is passed, treat the target's name as the src.
    #
    #     export_exe(
    #         name = "script.sh",
    #     )
    #
    # is equivalent to:
    #
    #     export_exe(
    #         name = "script.sh",
    #         src = "script.sh",
    #     )
    #
    if platform_exe == None and not (exe or src):
        src = name
    _export_exe(name = name, exe = exe, src = src, platform_exe = platform_exe, **kwargs)
