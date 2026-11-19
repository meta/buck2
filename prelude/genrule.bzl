# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Implementation of the `genrule` build rule.

load("@prelude//:cache_mode.bzl", "CacheModeInfo")
load("@prelude//:genrule_local_labels.bzl", "resolved_genrule_labels_require_local")
load("@prelude//:genrule_prefer_local_labels.bzl", "resolved_genrule_labels_prefer_local")
load("@prelude//:genrule_toolchain.bzl", "GenruleToolchainInfo")
load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:genrule_common.bzl", "genrule_common")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:utils.bzl", "flatten", "value_or")

GENRULE_OUT_DIR = "out"

# Currently, some rules require running from the project root, so provide an
# opt-in list for those here.  Longer-term, these should be ported to actual
# rule implementations in v2, rather then using `genrule`s.
# TODO: Roll out root based genrules everywhere and flip the default to get rid of this logic.
_BUILD_ROOT_LABELS = set([
    # The buck2 test suite
    "buck2_test_build_root",
    "antlir_macros",
    "rust_bindgen",
    "haskell_hsc",
    "cql_cxx_genrule",
    "clang-module",
    "bundle_pch_genrule",  # Compiles C++, and so need to run from build root
    "udf_sql",
    "redex_genrule",  # T148016945
    "pxl",  # T151533831
    "app_modules_genrule",  # produces JSON containing file paths that are read from the root dir.
    "android_langpack_strings",  # produces JSON containing file paths that are read from the root dir.
    "windows_long_path_issue",  # Windows: relative path length exceeds PATH_MAX, program cannot access file
    "flowtype_ota_safety_target",  # produces JSON containing file paths that are project-relative
    "ctrlr_setting_paths",
    "llvm_buck_genrule",
])

# In Buck1 the SRCS environment variable is only set if the substring SRCS is on the command line.
# That's a horrible heuristic, and doesn't account for users accessing $SRCS from a shell script.
# But in some cases, $SRCS is so large it breaks the process limit, so have a label to opt in to
# that behavior.
_NO_SRCS_ENVIRONMENT_LABEL = "no_srcs_environment"

_WINDOWS_ENV_SUBSTITUTIONS = [
    # Replace $OUT and ${OUT}
    (regex("\\$(OUT\\b|\\{OUT\\})"), "%OUT%"),
    (regex("\\$(SRCDIR\\b|\\{SRCDIR\\})"), "%SRCDIR%"),
    (regex("\\$(SRCS\\b|\\{SRCS\\})"), "%SRCS%"),
    (regex("\\$(TMP\\b|\\{TMP\\})"), "%TMP%"),
]

def _requires_build_root(ctx: AnalysisContext) -> bool:
    for label in ctx.attrs.labels:
        if label in _BUILD_ROOT_LABELS:
            return True
    if ctx.attrs.repo_relative_root:
        return True
    return False

def _requires_local(ctx: AnalysisContext) -> bool:
    return resolved_genrule_labels_require_local(ctx.attrs.labels)

def _prefers_local(ctx: AnalysisContext) -> bool:
    return resolved_genrule_labels_prefer_local(ctx.attrs.labels)

def _ignore_artifacts(ctx: AnalysisContext) -> bool:
    return "buck2_ignore_artifacts" in ctx.attrs.labels

def _requires_no_srcs_environment(ctx: AnalysisContext) -> bool:
    return _NO_SRCS_ENVIRONMENT_LABEL in ctx.attrs.labels

# We don't want to use cache mode in open source because the config keys that drive it aren't wired up
_USE_CACHE_MODE = is_full_meta_repo()

# Extra attributes required by every genrule based on genrule_impl
def genrule_attributes() -> dict[str, Attr]:
    attributes = {
        "allow_offline_output_cache": attrs.bool(default = False),
        "always_print_stderr": attrs.bool(default = False),
        "metadata_env_var": attrs.option(attrs.string(), default = None),
        "metadata_path": attrs.option(attrs.string(), default = None),
        "no_outputs_cleanup": attrs.bool(default = False),
        "remote_execution_dependencies": attrs.list(attrs.dict(key = attrs.string(), value = attrs.string()), default = []),
        "repo_relative_root": attrs.bool(
            default = False,
            doc = """
            If true, the genrule will be executed from the project root, instead of in the genrule location in buck-out.
            Helps with long paths issues on windows with deeply nested directories, which will usually have long relative paths as inputs.
            Should eventually default to true.
        """,
        ),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        "_genrule_toolchain": attrs.default_only(attrs.toolchain_dep(default = "toolchains//:genrule", providers = [GenruleToolchainInfo])),
    }

    if _USE_CACHE_MODE and not read_root_config("fb", "cache_mode") == None:
        attributes["_cache_mode"] = attrs.dep(default = read_root_config("fb", "cache_mode"))

    return attributes

def _get_cache_mode(ctx: AnalysisContext) -> CacheModeInfo:
    if _USE_CACHE_MODE:
        return ctx.attrs._cache_mode[CacheModeInfo]
    else:
        return CacheModeInfo(allow_cache_uploads = False, cache_bust_genrules = False)

def genrule_impl(ctx: AnalysisContext) -> list[Provider]:
    # Directories:
    #   sh - sh file
    #   src - sources files
    #   out - where outputs go
    # `src` is the current directory
    # Buck1 uses `.` as output, but that won't work since
    # Buck2 clears the output directory before execution, and thus src/sh too.
    return process_genrule(ctx, ctx.attrs.out, ctx.attrs.outs)

def _project_output(out: Artifact, path: str) -> Artifact:
    if path == ".":
        return out
    elif path.endswith("/"):
        return out.project(path[:-1], hide_prefix = True)
    else:
        return out.project(path, hide_prefix = True)

def _generate_error_handler(
    category: str, stderr_errorformats: list[str] | None, stdout_errorformats: list[str] | None
) -> typing.Callable[[ActionErrorCtx], list[ActionSubError]]:
    def handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
        structured_errors = []
        if stderr_errorformats != None:
            structured_errors = ctx.parse_with_errorformat(
                category = category,
                error = ctx.stderr,
                errorformats = stderr_errorformats,
            )

        if stdout_errorformats != None:
            errors = ctx.parse_with_errorformat(
                category = category,
                error = ctx.stdout,
                errorformats = stdout_errorformats,
            )
            structured_errors += errors
        return structured_errors

    return handler

_HEADER_EXTENSIONS = [
    ".h",
    ".hpp",
    ".hh",
    ".h++",
    ".hxx",
    ".cuh",
    ".inc",
    ".tcc",
]

def _is_header(path: str) -> bool:
    for ext in _HEADER_EXTENSIONS:
        if path.endswith(ext):
            return True
    return False

# Resolves whether a genrule's outputs should use content-based paths. When
# `has_content_based_path` is left unset, outputs consisting solely of headers
# default to content-based paths.
def _is_content_based(content_based: [bool, None], out_attr: [str, None], outs_attr: [dict, None]) -> bool:
    # An explicit setting always wins; avoid scanning outputs.
    if content_based != None:
        return content_based

    if out_attr != None:
        return _is_header(out_attr)

    if outs_attr == None:
        return False

    for paths in outs_attr.values():
        for path in paths:
            if not _is_header(path):
                return False
    return True

def process_genrule(
    ctx: AnalysisContext,
    out_attr: [str, None],
    outs_attr: [dict, None],
    extra_env_vars: dict = {},
    identifier: [str, None] = None,
    other_outputs: list[Artifact] = [],
    genrule_error_handler: [typing.Callable[[ActionErrorCtx], list[ActionSubError]], None] = None,
) -> list[Provider]:
    if (out_attr != None) and (outs_attr != None):
        fail("Only one of `out` and `outs` should be set. Got out=`%s`, outs=`%s`" % (repr(out_attr), repr(outs_attr)))

    local_only = _requires_local(ctx)
    prefer_local = _prefers_local(ctx)

    # NOTE: Eventually we shouldn't require local_only here, since we should be
    # fine with caching local fallbacks if necessary (or maybe that should be
    # disallowed as a matter of policy), but for now let's be safe.
    cacheable = value_or(ctx.attrs.cacheable, True) and (local_only or prefer_local)

    executable_outs = getattr(ctx.attrs, "executable_outs", None)

    content_based = _is_content_based(
        getattr(ctx.attrs, "has_content_based_path", None),
        out_attr,
        outs_attr,
    )

    # `out_dir_artifact`: The base artifact into which all the outputs go
    # `out_env`: The path we put into `$OUT`
    # `out_prepare`: The directory to `mkdir` before calling the user's code
    #
    # There is no justification for the particular choice of semantics today other than that's how
    # it always worked and changing it is very hard
    out_dir_artifact = ctx.actions.declare_output(GENRULE_OUT_DIR, dir = True, has_content_based_path = content_based)
    if out_attr != None:
        out_env = _project_output(out_dir_artifact, out_attr)
        if out_attr == ".":
            out_prepare = out_env.as_output()
        else:
            out_prepare = cmd_args(out_env.as_output(), parent = 1)
        named_outputs = {}
        default_outputs = [out_env]
        expect(executable_outs == None, "`executable_outs` should not be set when `out` is set")
    elif outs_attr != None:
        out_env = out_dir_artifact
        out_prepare = out_dir_artifact.as_output()
        named_outputs = {name: [_project_output(out_dir_artifact, path) for path in outputs] for (name, outputs) in outs_attr.items()}

        outs_names = outs_attr.keys()
        if executable_outs != None:
            for executable_out in executable_outs:
                expect(executable_out in outs_names, "Value in `executable_outs` {} is not in `outs`".format(executable_out))

        default_outputs = [_project_output(out_dir_artifact, path) for path in (ctx.attrs.default_outs or [])]
        if len(default_outputs) == 0:
            # We want building to force something to be built, so make sure it contains at least one artifact
            default_outputs = [out_dir_artifact]
    else:
        fail("One of `out` or `outs` should be set. Got `%s`" % repr(ctx.attrs))

    # Some custom rules use `process_genrule` but doesn't set this attribute.
    is_windows = hasattr(ctx.attrs, "_exec_os_type") and ctx.attrs._exec_os_type[OsLookup].os == Os("windows")
    if is_windows:
        path_sep = "\\"
        cmd = ctx.attrs.cmd_exe if ctx.attrs.cmd_exe != None else ctx.attrs.cmd
        if cmd == None:
            fail("One of `cmd` or `cmd_exe` should be set.")
    else:
        path_sep = "/"
        cmd = ctx.attrs.bash if ctx.attrs.bash != None else ctx.attrs.cmd
        if cmd == None:
            fail("One of `cmd` or `bash` should be set.")

    replace_regex = []

    # For backwards compatibility with Buck1.
    if is_windows:
        for re, sub in _WINDOWS_ENV_SUBSTITUTIONS:
            replace_regex.append((re, sub))

        for extra_env_var in extra_env_vars:
            replace_regex.append(
                (regex("\\$(%s\\b|\\{%s\\})" % (extra_env_var, extra_env_var)), "%%%s%%" % extra_env_var),
            )

    cmd = cmd_args(cmd, ignore_artifacts = _ignore_artifacts(ctx), replace_regex = replace_regex)

    if type(ctx.attrs.srcs) == type([]):
        # FIXME: We should always use the short_path, but currently that is sometimes blank.
        # See fbcode//buck2/tests/targets/rules/genrule:genrule-dot-input for a test that exposes it.
        symlinks = {src.short_path: src for src in ctx.attrs.srcs}

        if len(symlinks) != len(ctx.attrs.srcs):
            for src in ctx.attrs.srcs:
                name = src.short_path
                if symlinks[name] != src:
                    msg = "genrule srcs include duplicative name: `{}`. ".format(name)
                    msg += "`{}` conflicts with `{}`".format(symlinks[name].owner, src.owner)
                    fail(msg)
    else:
        symlinks = ctx.attrs.srcs
    srcs_artifact = ctx.actions.symlinked_dir(
        "srcs" if not identifier else "{}-srcs".format(identifier),
        symlinks,
        has_content_based_path = content_based,
    )

    if ctx.attrs.environment_expansion_separator:
        delimiter = ctx.attrs.environment_expansion_separator
    else:
        delimiter = " "

    # Setup environment variables.
    no_srcs_environment = _requires_no_srcs_environment(ctx)
    env_vars = {
        "OUT": out_env.as_output(),
        "SRCDIR": cmd_args(srcs_artifact, format = path_sep.join([".", "{}"])),
    }
    if not no_srcs_environment:
        srcs = cmd_args(delimiter = delimiter)
        for symlink in symlinks:
            srcs.add(cmd_args(srcs_artifact, format = path_sep.join([".", "{}", symlink.replace("/", path_sep)])))
        env_vars["SRCS"] = srcs
    env_vars |= {k: cmd_args(v) for k, v in getattr(ctx.attrs, "env", {}).items()}

    # RE will cache successful actions that don't produce the desired outptuts,
    # so if that happens and _then_ we add a local-only label, we'll get a
    # cache hit on the action that didn't produce the outputs and get the error
    # again (thus making the label useless). So, when a local-only label is
    # set, we make the action *different*.
    if local_only:
        env_vars["__BUCK2_LOCAL_ONLY_CACHE_BUSTER"] = ""

    # see comment above
    if prefer_local:
        env_vars["__BUCK2_PREFER_LOCAL_CACHE_BUSTER"] = ""

    # For now, when uploads are enabled, be safe and avoid sharing cache hits.
    cache_bust = _get_cache_mode(ctx).cache_bust_genrules

    if cacheable and cache_bust:
        env_vars["__BUCK2_ALLOW_CACHE_UPLOADS_CACHE_BUSTER"] = ""

    for key, value in extra_env_vars.items():
        env_vars[key] = value

    # Create required directories.
    if is_windows:
        script = [
            cmd_args(
                out_prepare,
                format = "if not exist {} mkdir {}",
            ),
            cmd_args('if NOT "%TEMP%" == "" set "TMP=%TEMP%"'),
        ]
        script_extension = "bat"
    else:
        script = [
            # Use a somewhat unique exit code so this can get retried on RE (T99656531).
            cmd_args(out_prepare, format = "mkdir -p {} || exit 99"),
            cmd_args("export TMP=${TMPDIR:-/tmp}"),
        ]
        script_extension = "sh"

    # Actually define the operation, relative to where we changed to
    script.append(cmd)

    hidden = []
    genrule_toolchain = ctx.attrs._genrule_toolchain[GenruleToolchainInfo]
    zip_scrubber = genrule_toolchain.zip_scrubber
    if not is_windows and zip_scrubber != None:
        zip_outputs = [output for output in default_outputs + flatten(named_outputs.values()) if output.extension == ".zip"]

        if zip_outputs:
            hidden.append(zip_scrubber)

            # Any outputs that are .zip files need to be "scrubbed" to ensure that they are deterministic.
            script = (
                [
                    cmd_args("ORIGINAL_DIR_FOR_ZIP_SCRUBBING=$(pwd)"),
                ]
                + script
                + [
                    cmd_args('cd "$ORIGINAL_DIR_FOR_ZIP_SCRUBBING"'),
                ]
                + [cmd_args(zip_scrubber, output.as_output(), delimiter = " ", quote = "shell") for output in zip_outputs]
            )

    # Some rules need to run from the build root, but for everything else, `cd`
    # into the sandboxed source dir and relative all paths to that.
    if not _requires_build_root(ctx):
        srcs_dir = srcs_artifact

        if is_windows:
            rewrite_scratch_path = cmd_args(
                cmd_args(ctx.label.project_root, relative_to = srcs_artifact),
                format = 'set "BUCK_SCRATCH_PATH={}\\%BUCK_SCRATCH_PATH%"',
            )
        else:
            srcs_dir = cmd_args(srcs_dir, quote = "shell")
            rewrite_scratch_path = cmd_args(
                cmd_args(ctx.label.project_root, quote = "shell", relative_to = srcs_artifact),
                format = "export BUCK_SCRATCH_PATH={}/$BUCK_SCRATCH_PATH",
            )

        # Relativize all paths in the command to the sandbox dir.
        for script_cmd in script:
            script_cmd.relative_to(srcs_artifact)

        script = [
            # Rewrite BUCK_SCRATCH_PATH
            rewrite_scratch_path,
            # Change to the directory that genrules expect.
            cmd_args(srcs_dir, format = "cd {}"),
        ] + script

        # Relative all paths in the env to the sandbox dir.
        env_vars = {key: cmd_args(value, relative_to = srcs_artifact) for key, value in env_vars.items()}

    if is_windows:
        # Should be in the beginning.
        # Odd, why is this a single string. How does it not end up getting quoted and being weird?
        script = [cmd_args("@echo off")] + script

    sh_script, _ = ctx.actions.write(
        "sh/genrule.{}".format(script_extension) if not identifier else "sh/{}-genrule.{}".format(identifier, script_extension),
        script,
        is_executable = True,
        allow_args = True,
        has_content_based_path = content_based,
    )
    if is_windows:
        script_args = ["cmd.exe", "/v:off", "/c", sh_script]
    else:
        script_args = ["/usr/bin/env", "bash", "-e", sh_script]

    # Only set metadata arguments when they are non-null
    metadata_args = {}
    if ctx.attrs.metadata_env_var:
        metadata_args["metadata_env_var"] = ctx.attrs.metadata_env_var
    if ctx.attrs.metadata_path:
        metadata_args["metadata_path"] = ctx.attrs.metadata_path
    if ctx.attrs.remote_execution_dependencies:
        metadata_args["remote_execution_dependencies"] = ctx.attrs.remote_execution_dependencies

    if genrule_error_handler == None:
        error_handler_category = getattr(ctx.attrs, "error_handler_category", None)
        error_handler_stderr_errorformats = getattr(ctx.attrs, "error_handler_stderr_errorformats", None)
        error_handler_stdout_errorformats = getattr(ctx.attrs, "error_handler_stdout_errorformats", None)
        if error_handler_category != None and (error_handler_stderr_errorformats != None or error_handler_stdout_errorformats != None):
            genrule_error_handler = _generate_error_handler(
                error_handler_category,
                error_handler_stderr_errorformats,
                error_handler_stdout_errorformats,
            )

    category = "genrule"
    if ctx.attrs.type != None:
        # As of 09/2021, all genrule types were legal snake case if their dashes and periods were replaced with underscores.
        category += "_" + ctx.attrs.type.replace("-", "_").replace(".", "_")
    ctx.actions.run(
        cmd_args(script_args, hidden = [cmd, srcs_artifact, out_dir_artifact.as_output()] + hidden),
        env = env_vars,
        local_only = local_only,
        prefer_local = prefer_local,
        weight = value_or(ctx.attrs.weight, 1),
        allow_cache_upload = cacheable,
        allow_offline_output_cache = ctx.attrs.allow_offline_output_cache,
        category = category,
        identifier = identifier,
        no_outputs_cleanup = ctx.attrs.no_outputs_cleanup,
        always_print_stderr = ctx.attrs.always_print_stderr,
        error_handler = genrule_error_handler,
        **metadata_args,
    )

    sub_targets = {}
    for k, v in named_outputs.items():
        sub_target_providers = [DefaultInfo(default_outputs = v)]
        if executable_outs != None and k in executable_outs:
            sub_target_providers.append(RunInfo(args = cmd_args(v)))
        sub_targets[k] = sub_target_providers

    providers = [
        DefaultInfo(
            default_outputs = default_outputs,
            sub_targets = sub_targets,
            other_outputs = other_outputs,
        )
    ]

    # The cxx_genrule also forwards here, and that doesn't have .executable, so use getattr
    if getattr(ctx.attrs, "executable", False):
        providers.append(RunInfo(args = cmd_args(default_outputs)))
    return providers

genrule = prelude_rule(
    name = "genrule",
    impl = genrule_impl,
    docs = """
        A `genrule()` is used to generate files from a shell
         command. It must produce a single output file or folder.
    """,
    examples = """
        This genrule() uses a Python script to derive a new
         `AndroidManifest.xml` from an
         `AndroidManifest.xml` in the source tree.
        Note you don't need to prepend execution commands with
         `python`: Buck knows how to execute different
        kinds of binaries using `$(exe)` command.

        ```
        genrule(
          name = 'generate_manifest',
          srcs = [
            'AndroidManifest.xml',
          ],
          bash = '$(exe //python/android:basic_to_full) ' \
              '$SRCDIR/AndroidManifest.xml > $OUT',
          cmd_exe = '$(exe //python/android:basic_to_full) ' \
              '%SRCDIR%\\AndroidManifest.xml > %OUT%',
          out = 'AndroidManifest.xml',
        )
        ```

        ```
        genrule(
          name = 'generate_manifest_with_named_outputs',
          srcs = [
            'AndroidManifest.xml',
          ],
          bash = '$(exe //python/android:basic_to_full) ' \
              '$SRCDIR/AndroidManifest.xml > $OUT/AndroidManifest.xml',
          cmd_exe = '$(exe //python/android:basic_to_full) ' \
              '%SRCDIR%\\AndroidManifest.xml > %OUT%\\AndroidManifest.xml',
          outs =  {
            "manifest": [ "AndroidManifest.xml" ],
          },
          default_outs = [ "AndroidManifest.xml" ],
        )
        ```

        For named outputs, build with any of the following:

        ```
          buck build //:generate_manifest_with_named_outputs
        ```

        ```
          buck build //:generate_manifest_with_named_outputs[manifest]
        ```

        Consume in `srcs` with:

        ```
        export_file(
            name = "magic1",
            src = ":generate_manifest_with_named_outputs",
            out = "some_dir_to_copy_to/AndroidManifest.xml",
        )
        ```

        ```
        export_file(
            name = "magic2",
            src = ":generate_manifest_with_named_outputs[manifest]",
            out = "some_dir_to_copy_to/AndroidManifest.xml",
        )
        ```

        Note that `magic1` consumes `generate_manifest_with_named_outputs`'s default
        output. `magic2` consumes `generate_manifest_with_named_outputs`'s named
        output "manifest," which happen to be pointing to the same output as the default output in this
        case, but they do not have to point to the same output.
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        genrule_common.srcs_arg()
        | genrule_common.cmd_arg()
        | genrule_common.bash_arg()
        | genrule_common.cmd_exe_arg()
        | genrule_common.type_arg()
        | genrule_common.weight_arg()
        | {
            "default_outs": attrs.option(
                attrs.set(attrs.string(), sorted = False),
                default = None,
                doc = """
                Default output which must be present if the `outs` arg is present. Otherwise does not apply.

                 If a rule with `outs` is consumed without an output label, the default output is returned. The
                 default output does not need to be present in any of the named outputs defined in `outs`.

                 Note that a maximum of one value may be present in this list. For example:

                ```
                default_outs = [ "output_one", ]
                ```
                is valid, whereas

                ```
                default_outs = [ "output_one", "output_two", ]
                ```
                is not.
            """,
            ),
            "executable_outs": attrs.option(
                attrs.set(attrs.string(), sorted = False),
                default = None,
                doc = """
                Only valid if the `outs` arg is present. Dictates which of those named outputs are marked as
                executable.
            """,
            ),
            "out": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                The name of the output file or directory. The complete path to this
                 argument is provided to the shell command through
                 the `OUT` environment variable. Only one of `out`
                 or `outs` may be present.
            """,
            ),
            "outs": attrs.option(
                attrs.dict(key = attrs.string(), value = attrs.set(attrs.string(), sorted = False), sorted = False),
                default = None,
                doc = """
                Mapping defining `named outputs`
                  to output paths relative to the rule's output directory. Only one of
                  `out` or `outs` may be present.

                 Example:

                ```
                genrule(
                  name = "named_outputs",
                  outs = {
                    "output1": [
                      "out1.txt",
                    ],
                    "output2": [
                      "out2.txt",
                    ],
                  },
                  default_outs = [ "out1.txt" ],
                  cmd = "echo something> $OUT/out1.txt && echo another> $OUT/out2.txt",
                )
                ```

                 Note that a maximum of one value may be present in the list in this map. For example:

                ```
                outs = {
                  "output1": [
                    "out1.txt",
                  ],
                },
                ```
                is valid, whereas

                ```
                outs = {
                  "output1": [
                    "out1.txt",
                    "out2.txt",
                  ],
                },
                ```
                is not.
            """,
            ),
        }
        | buck.optional_has_content_based_path_attr()
        | genrule_common.env_arg()
        | genrule_common.environment_expansion_separator()
        | {
            "cacheable": attrs.option(attrs.bool(), default = None),
            "enable_sandbox": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                Whether this target should be executed in a sandbox or not.
            """,
            ),
            "executable": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                Whether the output of the genrule is itself executable. Marking an output as
                 executable makes `buck run` and `$(exe ...)` macro
                 expansion work with this target.
            """,
            ),
            "remote": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                Opts this genrule in to remote execution. Note that it is only safe to
                 execute a genrule remotely if it is completely hermetic and completely
                 and correctly describes its dependencies. Defaults to false. This parameter
                 is unstable. It is subject to removal, default reversal, and other arbitrary
                 changes in the future.
            """,
            ),
            "_exec_os_type": buck.exec_os_type_arg(),
        }
        | genrule_common.error_handler_arg()
        | genrule_common.allow_offline_output_cache_arg()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | genrule_attributes()
        | constraint_overrides.attributes
    ),
    cfg = constraint_overrides.transition,
)
