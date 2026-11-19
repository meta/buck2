# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:platform_exe.bzl", "resolve_platform_exe")
load("@prelude//:platform_exe_constants.bzl", "SUPPORTED_ARCHS", "SUPPORTED_OSES")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//os_lookup:defs.bzl", "OsLookup", "ScriptLanguage")
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused: Used as a type

def command_alias_impl(ctx: AnalysisContext):
    target_os = ctx.attrs._target_os_type[OsLookup]

    base = resolve_platform_exe(ctx.attrs.exe, ctx.attrs.platform_exe, target_os)

    output = command_alias(
        actions = ctx.actions,
        path = ctx.attrs.executable_name,
        target_os = target_os,
        base = base,
        args = cmd_args(ctx.attrs.args),
        env = ctx.attrs.env,
        labels = ctx.attrs.labels,
    )

    # FIXME(JakobDegen): We should not accept `platform_exe` as meaning `run_using_single_arg`, but
    # there are things that depend on that
    #
    # FIXME(JakobDegen): It's easy to end up depending on either one of these behaviors. Life would
    # probably be easier if we just always went the `output.cmd` route
    return command_alias_providers(
        output,
        ctx.attrs.resources,
        run_with_trampoline = (
            ctx.attrs.run_using_single_arg
            or ctx.attrs.executable_name != None
            or (len(ctx.attrs.platform_exe) > 0 and target_os.script == ScriptLanguage("sh"))
        ),
    )

CommandAliasOutput = record(
    # A batch or sh script representing the command alias
    #
    # `DefaultInfo` instead of `Artifact` because the `other_outputs` will usually need to be
    # included as hidden somewhere
    output = DefaultInfo,
    # The output wrapped into a `cmd_args` in the obvious way
    cmd = cmd_args,
    # For some values of the arguments to `command_alias` it is possible to represent the
    # command_alias entirely within a `cmd_args`, without needing to write out a script. If that is
    # the case, those args are made available here.
    maybe_directly_runnable = cmd_args | None,
)

def command_alias_providers(
    output: CommandAliasOutput, resources: list[Artifact], run_with_trampoline: bool = False, sub_targets: dict[str, list[Provider]] = {}
) -> list[Provider]:
    """The `DefaultInfo` and `RunInfo` of a `command_alias` target for `output`.

    The trampoline is the default output. `resources` are other outputs and hidden inputs of
    `RunInfo`.
    """
    default_info = DefaultInfo(
        default_output = output.output.default_outputs[0],
        other_outputs = list(output.output.other_outputs) + resources,
        sub_targets = sub_targets,
    )
    if output.maybe_directly_runnable == None or run_with_trampoline:
        run_info = RunInfo(args = output.cmd)
    else:
        run_info = RunInfo(args = output.maybe_directly_runnable)

    run_info_with_resources = RunInfo(
        args = cmd_args(run_info, hidden = resources),
    )

    return [default_info, run_info_with_resources]

def command_alias(
    *,
    actions: AnalysisActions,
    # The path at which to write the output to, without an extension - that will be added
    path: str | None,
    # The target where this script should be able to run (this may actually be your exec platform)
    target_os: OsLookup,
    # Either the `RunInfo` to use, or in the case of a fat platform, the choice of `RunInfo`
    # depending on `uname`
    base: RunInfo | dict[str, RunInfo | dict[str, RunInfo]],
    args: cmd_args,
    env: dict[str, ArgLike],
    labels: list[str],
    has_content_based_path: bool = False,
) -> CommandAliasOutput:
    if path == "":
        fail("Path cannot be empty string")

    if path != None:
        # Custom paths are usually provided for binaries that are searched for with a particular
        # name. On Windows, the .bat extension is required, and well-behaved tools will respect the
        # PATHEXT environment variable (which contains that extension). On Unix, no extension is
        # expected for executables, and searches would likely fail if we added one.
        unix_trampoline_path = path
        windows_trampoline_path = path + ".bat"
    else:
        # Preserve the `.sh` extension in the default path, since many places have it hard-coded.
        unix_trampoline_path = "__command_alias_trampoline.sh"
        windows_trampoline_path = "__command_alias_trampoline.bat"

    if target_os.script == ScriptLanguage("sh"):
        trampoline, hidden = _command_alias_write_trampoline_unix(actions, unix_trampoline_path, base, args, env, has_content_based_path)
    elif target_os.script == ScriptLanguage("bat"):
        trampoline, hidden = _command_alias_write_trampoline_windows(actions, windows_trampoline_path, base, args, env, labels, has_content_based_path)
    else:
        fail("Unsupported script language: {}".format(target_os.script))

    if len(env) > 0 or isinstance(base, dict):
        maybe_directly_runnable = None
    else:
        maybe_directly_runnable = cmd_args(base.args, args)

    return CommandAliasOutput(
        output = DefaultInfo(
            default_output = trampoline,
            other_outputs = [hidden],
        ),
        cmd = cmd_args(trampoline, hidden = hidden),
        maybe_directly_runnable = maybe_directly_runnable,
    )

def _command_alias_write_trampoline_unix(
    actions: AnalysisActions,
    path: str,
    base: RunInfo | dict[str, RunInfo | dict[str, RunInfo]],
    args: cmd_args,
    env: dict[str, ArgLike],
    has_content_based_path: bool,
) -> (Artifact, cmd_args):
    trampoline_args = cmd_args()
    trampoline_args.add("#!/usr/bin/env bash")
    trampoline_args.add("set -euo pipefail")

    if isinstance(base, dict):
        os_cases = []
        for uname, run_info in base.items():
            os_command = cmd_args()
            if isinstance(run_info, dict):
                arch_cases = []
                for arch, arch_run_info in run_info.items():
                    # Direct callers can supply maps that bypass resolve_platform_exe.
                    if arch not in SUPPORTED_ARCHS:
                        fail("Unsupported command_alias runtime arch: {}; expected {}".format(arch, SUPPORTED_ARCHS))
                    arch_command = cmd_args()
                    _add_args_declaration_to_trampoline_args(arch_command, arch_run_info, args)
                    arch_cases.append(_trampoline_case("arm64|aarch64" if arch == "arm64" else arch, arch_command))
                os_command.add(
                    cmd_args(
                        cmd_args(arch_cases, delimiter = "\n"),
                        format = """case "$(uname -m)" in
{}
    *) echo "Unsupported architecture for command_alias: $(uname -m)" >&2; exit 1 ;;
esac""",
                    )
                )
            else:
                _add_args_declaration_to_trampoline_args(os_command, run_info, args)
            os_cases.append(_trampoline_case(uname, os_command))
        trampoline_args.add(
            cmd_args(
                cmd_args(os_cases, delimiter = "\n"),
                format = """case "$(uname)" in
{}
    *) echo "Unsupported operating system for command_alias: $(uname)" >&2; exit 1 ;;
esac""",
            )
        )
    else:
        _add_args_declaration_to_trampoline_args(trampoline_args, base, args)

    # We can't use cwd relative paths (since we don't know the cwd when this script is run) and so
    # we instead use paths relative to the script itself. However, we can't just naively stick a
    # `$(...)` into `absolute_prefix`, since we must also shell quote paths, and that expression
    # would be shell quoted.
    #
    # Instead, we use `BUCK_COMMAND_ALIAS_ABSOLUTE_PREFIX/`, verbatim, as an absolute prefix on the
    # cmd_args, and then replace that with the actual path of the script at runtime
    #
    # Resolve symlinks first to handle execution via symlinks (e.g., in link-trees)
    trampoline_args.add(
        """
SCRIPT_PATH="$0"
if [ -L "$SCRIPT_PATH" ]; then
    TARGET="$(readlink "$SCRIPT_PATH")"
    SCRIPT_PATH="$(dirname "$SCRIPT_PATH")/$TARGET"
fi
""",
    )

    # Calculate the base path and process arguments with the resolved path.
    trampoline_args.add(
        """
BASE=$(cd -- "$(dirname "$SCRIPT_PATH")" >/dev/null 2>&1 ; pwd -P)
R_ARGS=()
for arg in "${ARGS[@]}"; do
    # Normalize Windows-style backslash path separators to forward slashes for
    # artifact paths (identified by the prefix placeholder). This handles the case
    # where Buck2 runs on Windows but this trampoline executes on Linux via RE.
    if [[ "$arg" == *BUCK_COMMAND_ALIAS_ABSOLUTE_PREFIX* ]]; then arg="${arg//\\\\//}"; fi
    R_ARGS+=("${arg//BUCK_COMMAND_ALIAS_ABSOLUTE_PREFIX/$BASE}")
done
""",
    )

    for k, v in env.items():
        # TODO(akozhevnikov): maybe check environment variable is not conflicting with pre-existing one
        trampoline_args.add(cmd_args("export ", k, "=", cmd_args(v, quote = "shell"), delimiter = ""))
        trampoline_args.add(cmd_args("export ", k, '="${', k, '//BUCK_COMMAND_ALIAS_ABSOLUTE_PREFIX/$BASE}"', delimiter = ""))

    trampoline_args.add('exec "${R_ARGS[@]}" "$@"')

    trampoline = actions.declare_output(path, has_content_based_path = has_content_based_path)
    trampoline_args = cmd_args(
        trampoline_args,
        relative_to = (trampoline, 1),
        absolute_prefix = "BUCK_COMMAND_ALIAS_ABSOLUTE_PREFIX/",
    )
    actions.write(
        trampoline.as_output(),
        trampoline_args,
        allow_args = True,
        is_executable = True,
        has_content_based_path = has_content_based_path,
    )

    return trampoline, trampoline_args

def _command_alias_write_trampoline_windows(
    actions: AnalysisActions, path: str, base: RunInfo, args: cmd_args, env: dict[str, ArgLike], labels: list[str], has_content_based_path: bool
) -> (Artifact, cmd_args):
    trampoline_args = cmd_args()
    trampoline_args.add("@echo off")

    if "close_stdin" in labels:
        # Avoids waiting for input on the "Terminate batch job (Y/N)?" prompt.
        # The prompt itself is unavoidable, but we can avoid having to wait for input.
        # This will call the same trampoline batch file with stdin disabled
        trampoline_args.add("if not defined STDIN_CLOSED (set STDIN_CLOSED=1 & CALL <NUL %0 %* & GOTO :EOF)")

    # Set BUCK_COMMAND_ALIAS_ABSOLUTE to the drive and full path of the script being created here
    # We use this below to prefix any artifacts being referenced in the script
    trampoline_args.add("set BUCK_COMMAND_ALIAS_ABSOLUTE=%~dp0")

    # Handle envs
    for k, v in env.items():
        # TODO(akozhevnikov): maybe check environment variable is not conflicting with pre-existing one
        trampoline_args.add(cmd_args(["set ", k, "=", v], delimiter = ""))

    # FIXME(JakobDegen): This should be batch quoting, not shell quoting
    cmd = cmd_args(cmd_args(base.args, args, quote = "shell"), "%*", delimiter = " ")

    trampoline_args.add(cmd)

    trampoline = actions.declare_output(path, has_content_based_path = has_content_based_path)
    trampoline_args = cmd_args(
        trampoline_args,
        relative_to = (trampoline, 1),
        absolute_prefix = "%BUCK_COMMAND_ALIAS_ABSOLUTE%/",
    )
    actions.write(
        trampoline.as_output(),
        trampoline_args,
        allow_args = True,
        is_executable = True,
        has_content_based_path = has_content_based_path,
    )

    return trampoline, trampoline_args

def _trampoline_case(pattern: str, command: cmd_args) -> cmd_args:
    return cmd_args(
        cmd_args(command, delimiter = "\n"),
        format = """    {pattern})
{{}}
        ;;""".format(pattern = pattern),
    )

def _add_args_declaration_to_trampoline_args(trampoline_args: cmd_args, base: RunInfo, args: cmd_args):
    trampoline_args.add("ARGS=(")
    trampoline_args.add(cmd_args(base.args, args, quote = "shell"))
    trampoline_args.add(")")

# `command_alias` is the trampoline-writing helper above, hence the suffix.
command_alias_rule = prelude_rule(
    name = "command_alias",
    impl = command_alias_impl,
    docs = """
        The `command_alias` rule enables you to wrap build
         rules that create binaries and to pre-apply command-line
         arguments and environment variables.

         Example uses include running a command written in a scripting
         language with a specific interpreter, and transparently wrapping
         sub-commands of a binary.

         You can reference a `command_alias` target in
         the `cmd` parameter of a `genrule()` by
         using the `exe` macro:

        ```
        $(exe //path/to:target)
        ```
    """,
    examples = """
        ```
        # Combining an interpreter and a script

        cxx_binary(
            name = "node-js",
            srcs = [
                # ...
            ],
            headers = [
                # ...
            ],
        )

        export_file(
            name = "scripts"
        )

        command_alias(
            name = "server",
            exe = ":node-js",
            args = [
                "$(location :scripts)/start-server.js",
            ],
        )
        ```

        ```
        # Exposing sub commands

        export_file(
            name = "yarn",
            src = "yarn.sh",
        )

        command_alias(
            name = "add",
            exe = ":yarn",
            args = ["add"],
        )

        command_alias(
            name = "install",
            exe = ":yarn",
            args = ["install"],
        )

        command_alias(
            name = "run",
            exe = ":yarn",
            args = ["run"],
        )
        ```

        ```
        # Platform specific commands

        export_file(
            name = "node-windows",
            src = "windows/node.exe",
        )

        export_file(
            name = "node-linux",
            src = "linux/node",
        )

        export_file(
            name = "node-macos",
            src = "macos/node",
        )

        command_alias(
            name = "node",
            platform_exe = {
                "windows": ":node-windows",
                "linux": ":node-linux",
                "macos": ":node-macos",
            },
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            # Match `dep` before `source` so that we can support extracting the
            # `RunInfo` provider of it, if one exists.
            "exe": attrs.option(
                attrs.one_of(attrs.dep(), attrs.source()),
                default = None,
                doc = """
                A `build target` for a rule that outputs
                 an executable, such as an `sh_binary()`,
                 or an executable source file.
            """,
            ),
            "platform_exe": attrs.dict(
                key = attrs.enum(SUPPORTED_OSES),
                value = attrs.one_of(
                    attrs.dep(),
                    attrs.dict(key = attrs.enum(SUPPORTED_ARCHS), value = attrs.dep(), sorted = False),
                ),
                sorted = False,
                default = {},
                doc = """
                A mapping from OS names (`linux`, `macos`, `windows`) to executable
                 dependencies or arch maps, for example:
                 `{"linux": {"x86_64": ":tool-x86", "arm64": ":tool-arm"}, "macos": ":universal"}`.

                Arch maps support `x86_64` and `arm64`. Thin configurations select the
                 target arch. Fat linux configurations require both Linux branches
                 and dispatch at runtime using `uname -m` (`aarch64` is treated as `arm64`).

                Fat mac/linux configurations dispatch only the OS at runtime. Arch maps
                 for both OSes select the same configured target arch, which must be
                 `x86_64` or `arm64`. For hosts with different architectures, use flat
                 entries supplying a suitable portable executable for each OS.

                Each mapped target must already build or supply an executable for the
                 named architecture. Adding an `arm64` key does not make Buck compile
                 that target for ARM. Buck may configure mapped dependencies even when
                 they are not selected. All arch maps must be nonempty and use supported
                 keys, including maps for unselected OSes.

                `exe` is an optional fallback for omitted OS entries, not missing arch
                 entries. Without it, an omitted OS is an error if any entry is an arch
                 map. With only flat entries, `args` supplies the command for an omitted OS.
            """,
            ),
            "args": attrs.list(
                attrs.arg(),
                default = [],
                doc = """
                A string of arguments that is passed to the executable specified by
                 `exe` at startup. These arguments support a subset of
                 Buck's `string parameter macros`
                . Only the
                 `$(location ...)` and `$(exe ...)` macros are supported currently.
            """,
            ),
            "env": attrs.dict(
                key = attrs.string(),
                value = attrs.arg(),
                sorted = False,
                default = {},
                doc = """
                A map of environment variables that will be passed to the executable represented
                 by `exe` on startup. Environment variables support the same macros as arguments.
            """,
            ),
            "executable_name": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                If provided, use this name for the trampoline script (with an extension added if
                 required by the platform).
            """,
            ),
            "resources": attrs.list(attrs.source(), default = []),
            "run_using_single_arg": attrs.bool(
                default = False,
                doc = """
                Ensure that the command alias can be run as a single argument (instead of
                $(exe) or RunInfo potentially expanding to multiple arguments).
            """,
            ),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_target_os_type": buck.target_os_type_arg(),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
