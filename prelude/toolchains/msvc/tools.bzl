# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerType")
load("@prelude//os_lookup:defs.bzl", "ScriptLanguage")
load("@prelude//toolchains:cxx.bzl", "CxxToolsInfo")
load("@prelude//utils:cmd_script.bzl", "cmd_script")

def _find_msvc_tools_impl(ctx: AnalysisContext) -> list[Provider]:
    cl_exe_json = ctx.actions.declare_output("cl.exe.json", has_content_based_path = False)
    cvtres_exe_json = ctx.actions.declare_output("cvtres.exe.json", has_content_based_path = False)
    lib_exe_json = ctx.actions.declare_output("lib.exe.json", has_content_based_path = False)
    ml64_exe_json = ctx.actions.declare_output("ml64.exe.json", has_content_based_path = False)
    link_exe_json = ctx.actions.declare_output("link.exe.json", has_content_based_path = False)
    rc_exe_json = ctx.actions.declare_output("rc.exe.json", has_content_based_path = False)

    cmd = [
        ctx.attrs.vswhere[RunInfo],
        cmd_args("--cl=", cl_exe_json.as_output(), delimiter = ""),
        cmd_args("--cvtres=", cvtres_exe_json.as_output(), delimiter = ""),
        cmd_args("--lib=", lib_exe_json.as_output(), delimiter = ""),
        cmd_args("--ml64=", ml64_exe_json.as_output(), delimiter = ""),
        cmd_args("--link=", link_exe_json.as_output(), delimiter = ""),
        cmd_args("--rc=", rc_exe_json.as_output(), delimiter = ""),
    ]

    ctx.actions.run(
        cmd,
        category = "vswhere",
        local_only = True,
    )

    run_msvc_tool = ctx.attrs.run_msvc_tool[RunInfo]
    if ctx.attrs.use_path_compilers:
        cl_exe_script = "cl.exe"
        ml64_exe_script = "ml64.exe"
        rc_exe_script = "rc.exe"
        cvtres_exe_script = "cvtres.exe"
    else:
        cl_exe_script = cmd_script(
            actions = ctx.actions,
            name = "cl",
            cmd = cmd_args(run_msvc_tool, cl_exe_json),
            language = ScriptLanguage("bat"),
        )
        cvtres_exe_script = cmd_script(
            actions = ctx.actions,
            name = "cvtres",
            cmd = cmd_args(run_msvc_tool, cvtres_exe_json),
            language = ScriptLanguage("bat"),
        )
        ml64_exe_script = cmd_script(
            actions = ctx.actions,
            name = "ml64",
            cmd = cmd_args(run_msvc_tool, ml64_exe_json),
            language = ScriptLanguage("bat"),
        )
        rc_exe_script = cmd_script(
            actions = ctx.actions,
            name = "rc",
            cmd = cmd_args(run_msvc_tool, rc_exe_json),
            language = ScriptLanguage("bat"),
        )

    if ctx.attrs.use_path_linkers:
        lib_exe_script = "lib.exe"
        link_exe_script = "link.exe"
    else:
        lib_exe_script = cmd_script(
            actions = ctx.actions,
            name = "lib",
            cmd = cmd_args(run_msvc_tool, lib_exe_json),
            language = ScriptLanguage("bat"),
        )
        link_exe_script = cmd_script(
            actions = ctx.actions,
            name = "link",
            cmd = cmd_args(run_msvc_tool, link_exe_json),
            language = ScriptLanguage("bat"),
        )

    return [
        # Supports `buck2 run prelude//toolchains/msvc:msvc_tools[cl.exe]`
        # and `buck2 build prelude//toolchains/msvc:msvc_tools[cl.exe][json]`
        DefaultInfo(
            sub_targets = {
                "cl.exe": [
                    RunInfo(args = [cl_exe_script]),
                    DefaultInfo(
                        sub_targets = {
                            "json": [DefaultInfo(default_output = cl_exe_json)],
                        }
                    ),
                ],
                "cvtres.exe": [
                    RunInfo(args = [cvtres_exe_script]),
                    DefaultInfo(
                        sub_targets = {
                            "json": [DefaultInfo(default_output = cvtres_exe_json)],
                        }
                    ),
                ],
                "lib.exe": [
                    RunInfo(args = [lib_exe_script]),
                    DefaultInfo(
                        sub_targets = {
                            "json": [DefaultInfo(default_output = lib_exe_json)],
                        }
                    ),
                ],
                "link.exe": [
                    RunInfo(args = [link_exe_script]),
                    DefaultInfo(
                        sub_targets = {
                            "json": [DefaultInfo(default_output = link_exe_json)],
                        }
                    ),
                ],
                "ml64.exe": [
                    RunInfo(args = [ml64_exe_script]),
                    DefaultInfo(
                        sub_targets = {
                            "json": [DefaultInfo(default_output = ml64_exe_json)],
                        }
                    ),
                ],
                "rc.exe": [
                    RunInfo(args = [rc_exe_script]),
                    DefaultInfo(
                        sub_targets = {
                            "json": [DefaultInfo(default_output = rc_exe_json)],
                        }
                    ),
                ],
            }
        ),
        CxxToolsInfo(
            compiler = cl_exe_script,
            compiler_type = "windows",
            cxx_compiler = cl_exe_script,
            asm_compiler = ml64_exe_script,
            asm_compiler_type = "windows_ml64",
            rc_compiler = rc_exe_script,
            cvtres_compiler = cvtres_exe_script,
            archiver = lib_exe_script,
            archiver_type = "windows",
            linker = _windows_linker_wrapper(ctx, link_exe_script),
            linker_type = LinkerType("windows"),
        ),
    ]

def _windows_linker_wrapper(ctx: AnalysisContext, linker: [cmd_args, str]) -> cmd_args:
    # Linkers pretty much all support @file.txt argument syntax to insert
    # arguments from the given text file, usually formatted one argument per
    # line.
    #
    # - GNU ld: https://gcc.gnu.org/onlinedocs/gcc/Overall-Options.html
    # - lld is command line compatible with GNU ld
    # - MSVC link.exe: https://learn.microsoft.com/en-us/cpp/build/reference/linking?view=msvc-170#link-command-files
    #
    # However, there is inconsistency in whether they support nesting of @file
    # arguments inside of another @file.
    #
    # We wrap the linker to flatten @file arguments down to 1 level of nesting.
    return cmd_script(
        actions = ctx.actions,
        name = "windows_linker",
        cmd = cmd_args(
            ctx.attrs.linker_wrapper[RunInfo],
            linker,
        ),
        language = ScriptLanguage("bat"),
    )

find_msvc_tools = rule(
    impl = _find_msvc_tools_impl,
    attrs = {
        "linker_wrapper": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//cxx/tools:linker_wrapper")),
        "run_msvc_tool": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//toolchains/msvc:run_msvc_tool")),
        "use_path_compilers": attrs.bool(default = False),
        "use_path_linkers": attrs.bool(default = False),
        "vswhere": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//toolchains/msvc:vswhere")),
    },
)
