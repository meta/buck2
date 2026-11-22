# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//utils:actions.bzl", "ActionExecutionAttributes")
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused Used as a type
load("@prelude//utils:script.bzl", "write_script")
load(":debug.bzl", "SplitDebugMode")

def _dwp_batch_arg(arg):
    return cmd_args(
        arg,
        replace_regex = [
            (regex('(\\\\*)"'), '${1}${1}""'),
            (regex("(\\\\+)$"), "${1}${1}"),
            ("%", "%%"),
        ],
        format = '"{}"',
    )

def _dwp_batch_command(command):
    return cmd_args(_dwp_batch_arg(command), delimiter = " ")

def dwp_available(toolchain: CxxToolchainInfo):
    dwp = toolchain.binary_utilities_info.dwp
    split_debug_mode = toolchain.split_debug_mode
    return dwp != None and split_debug_mode != SplitDebugMode("none")

def run_dwp_action(
    ctx: AnalysisContext,
    toolchain: CxxToolchainInfo,
    obj: Artifact,
    identifier: [str, None],
    category_suffix: [str, None],
    referenced_objects: [ArgLike, list[Artifact]],
    dwp_output: Artifact,
    action_execution_properties: ActionExecutionAttributes = ActionExecutionAttributes(),
    from_exe = True,
):
    dwp = toolchain.binary_utilities_info.dwp
    script_name = "dwp{}{}".format(
        "_" + category_suffix if category_suffix else "",
        "_" + identifier if identifier else "",
    )

    if from_exe:
        language = toolchain.internal_tools.script_language
        primary = cmd_args([dwp, "-o", dwp_output.as_output(), "-e", obj] + getattr(ctx.attrs, "extra_dwp_flags", []))
        fallback = cmd_args([dwp, "-o", dwp_output.as_output(), obj] + getattr(ctx.attrs, "extra_dwp_flags", []))
        if language == "sh":
            helper_scripts = []
            script = [
                cmd_args(primary, delimiter = " ", quote = "shell"),
                cmd_args(cmd_args(dwp_output.as_output(), quote = "shell"), format = "if [ ! -e {} ]; then"),
                cmd_args(fallback, delimiter = " ", quote = "shell"),
                "fi",
                cmd_args(cmd_args(dwp_output.as_output(), quote = "shell"), format = "test -e {}"),
            ]
        elif language == "bat":
            primary_script, _ = ctx.actions.write(
                script_name + ".primary.bat",
                ["@echo off", _dwp_batch_command(primary)],
                allow_args = True,
                has_content_based_path = False,
            )
            fallback_script, _ = ctx.actions.write(
                script_name + ".fallback.bat",
                ["@echo off", _dwp_batch_command(fallback)],
                allow_args = True,
                has_content_based_path = False,
            )
            script = [
                "@echo off",
                cmd_args(_dwp_batch_arg(primary_script), format = "cmd.exe /v:off /c {}"),
                'if not "%errorlevel%"=="0" exit /b %errorlevel%',
                cmd_args(_dwp_batch_arg(dwp_output.as_output()), format = "if exist {} exit /b 0"),
                cmd_args(_dwp_batch_arg(fallback_script), format = "cmd.exe /v:off /c {}"),
                'if not "%errorlevel%"=="0" exit /b %errorlevel%',
                cmd_args(_dwp_batch_arg(dwp_output.as_output()), format = "if exist {} exit /b 0"),
                "exit /b 1",
            ]
            helper_scripts = [primary_script, fallback_script]
        else:
            fail("Unsupported DWP script language: {}".format(language))
        args = cmd_args(
            write_script(ctx.actions, script_name, script, language),
            hidden = [primary, fallback, referenced_objects] + helper_scripts,
        )
    else:
        args = cmd_args(
            [dwp, "-o", dwp_output.as_output()] + getattr(ctx.attrs, "extra_dwp_flags", []),
        )
        argsfile, _ = ctx.actions.write(
            script_name + ".argsfile",
            referenced_objects,
            allow_args = True,
            has_content_based_path = False,
        )
        args.add(cmd_args(argsfile, format = "@{}", hidden = referenced_objects))

    category = "dwp"
    if category_suffix != None:
        category += "_" + category_suffix

    ctx.actions.run(
        args,
        category = category,
        identifier = identifier,
        prefer_local = action_execution_properties.prefer_local,
        prefer_remote = action_execution_properties.prefer_remote,
        local_only = action_execution_properties.local_only,
        force_full_hybrid_if_capable = action_execution_properties.full_hybrid,
        # dwp merges the debug info of the corpus the link consumed and its
        # memory scales the same way, so share the link's local-slot weight.
        weight = toolchain.linker_info.link_weight,
    )

def dwp(
    ctx: AnalysisContext,
    toolchain: CxxToolchainInfo,
    # Executable/library to extra dwo paths from.
    obj: Artifact,
    # An identifier that will uniquely name this link action in the context of a category. Useful for
    # differentiating multiple link actions in the same rule.
    identifier: [str, None],
    # A category suffix that will be added to the category of the link action that is generated.
    category_suffix: [str, None],
    # All `.o`/`.dwo` paths referenced in `obj`.
    # TODO(T110378122): Ideally, referenced objects are a list of artifacts,
    # but currently we don't track them properly.  So, we just pass in the full
    # link line and extract all inputs from that, which is a bit of an
    # overspecification.
    referenced_objects: [ArgLike, list[Artifact]],
    name_suffix: str = "",
    action_execution_properties: ActionExecutionAttributes = ActionExecutionAttributes(),
    from_exe = True,
) -> Artifact:
    # gdb/lldb expect to find a file named $file.dwp next to $file.
    output = ctx.actions.declare_output(obj.short_path + name_suffix + ".dwp", has_content_based_path = False)
    run_dwp_action(
        ctx,
        toolchain,
        obj,
        identifier,
        category_suffix,
        referenced_objects,
        output,
        action_execution_properties = action_execution_properties,
        from_exe = from_exe,
    )
    return output
