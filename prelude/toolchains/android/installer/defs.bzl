# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def android_installer_args(adb: str | None, adb_restart_on_failure: str | None, staged_install_mode: str | None) -> list[str]:
    """Returns the Android installer flags for the values of the buckconfig keys `android.adb`,
    `adb.adb_restart_on_failure` and `adb.staged_install_mode`; `None` means the key is unset.

    Read the keys in the BUCK file that calls this. Starlark runs a constant-argument
    `read_root_config` in a function body when the module compiles, so the key would become a
    dependency of every package that loads the module.
    """
    args = []
    if adb:
        args += ["--adb-executable-path", adb]
    if adb_restart_on_failure != None:
        args += ["--adb-restart-on-failure", _bool(adb_restart_on_failure)]
    if staged_install_mode != None:
        args += ["--staged-install-mode", _bool(staged_install_mode)]
    return args

def _bool(value: str) -> str:
    # The installer's flags also take `yes`, `on` and `1` as true and reject other values. These
    # keys keep the meaning `Boolean.parseBoolean` gave them: only `true`, in any case, is true.
    return "true" if value.lower() == "true" else "false"

def _installer_with_args_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        ctx.attrs.installer[DefaultInfo],
        RunInfo(args = cmd_args(ctx.attrs.installer[RunInfo], ctx.attrs.args)),
    ]

# Runs `installer` with `args` after its own arguments. The arguments are plain strings, so a
# `$(...)` in a buckconfig value is passed as it is, not expanded as a macro.
installer_with_args = rule(
    impl = _installer_with_args_impl,
    attrs = {
        "args": attrs.list(attrs.string(), default = []),
        "installer": attrs.dep(providers = [RunInfo]),
    },
)
