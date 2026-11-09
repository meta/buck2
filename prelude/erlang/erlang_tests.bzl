# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//tests:re_utils.bzl", "get_re_executors_from_props")
load(":attrs.bzl", "erlang_rule_attributes")
load(
    ":erlang_build.bzl",
    "erlang_build",
    "module_name",
)
load(":erlang_dependencies.bzl", "erlang_deps_rule")
load(":erlang_info.bzl", "CodePathEntry", "CodePathTSet", "ErlangAppInfo", "ErlangAppOrTestInfo", "ErlangDependencyInfo", "ErlangTestInfo", "code_path_args")
load(":erlang_otp_application.bzl", "normalize_application")
load(":erlang_paths.bzl", "basename_without_extension")
load(":erlang_shell.bzl", "erlang_shell")
load(
    ":erlang_toolchain.bzl",
    "get_toolchain",
)
load(
    ":erlang_utils.bzl",
    "file_mapping",
    "preserve_structure",
)

def erlang_tests_macro(
    erlang_app_rule,
    erlang_test_rule,
    suites: list[str],
    deps: list[str] = [],
    resources: list[str] = [],
    srcs: list[str] = [],
    prefix: str | None = None,
    generated_app_labels: list[str] = [],
    erl_opts: list[str] | Select | None = None,
    **common_attributes,
) -> None:
    """
    Generate multiple erlang_test targets based on the `suites` field.
    Also adds the default 'config' and 'deps' from the buck2 config.
    The macro also produces and adds
    resource targets for files in the suite associated <suitename>_data folder.
    """
    if not suites:
        return

    deps = [normalize_application(dep) for dep in deps]

    if srcs:
        # There is no "good name" for the application
        # We create one using the first suite from the list
        suite_name = basename_without_extension(suites[0])
        srcs_app = "{}_app".format(suite_name)
        app_deps = [dep for dep in deps if not dep.endswith("_SUITE")]
        erlang_app_rule(
            name = srcs_app,
            srcs = srcs,
            labels = generated_app_labels,
            applications = app_deps,
            erl_opts = erl_opts,
        )
        deps.append(":{}".format(srcs_app))

    for suite in suites:
        # forward resources and deps fields and generate erlang_test target
        suite_name = normalize_suite_name(basename_without_extension(suite))
        if not suite_name.endswith("_SUITE"):
            fail("erlang_tests target only accept suite as input, found " + suite_name)

        # check if there is a data folder and add it as resource if existing
        data_dir_name = "{}_data".format(suite_name)
        suite_resource = resources
        data_target = generate_file_map_target(suite, prefix, data_dir_name)
        if data_target != None:
            suite_resource = list(suite_resource)  # copy
            suite_resource.append(data_target)

        if prefix != None:
            suite_name = "{}_{}".format(prefix, suite_name)

        # forward resources and deps fields and generate erlang_test target
        erlang_test_rule(name = suite_name, suite = suite, deps = deps, resources = suite_resource, erl_opts = erl_opts, **common_attributes)

def normalize_suite_name(suite_name: str) -> str:
    return suite_name.split(":")[-1]

default_test_args = cmd_args(
    "-mode",
    "minimal",
    "-noinput",
    "-noshell",
    "+A0",
    "+S1:1",
    "+sbtu",
    "-run",
    "test_binary",  # provided by ctx.attr._test_binary_lib
    "main",
)

def erlang_test_impl(ctx: AnalysisContext) -> Promise:
    # collect all dependencies
    dep_info_rule = (erlang_deps_rule, {"deps": ctx.attrs.deps + [ctx.attrs._test_binary_lib]})
    binary_lib_dep_info_rule = (erlang_deps_rule, {"deps": [ctx.attrs._test_binary_lib]})

    dep_infos = ctx.actions.anon_targets([dep_info_rule, binary_lib_dep_info_rule])

    return dep_infos.promise.map(
        lambda dep_infos: _build_erlang_test(
            ctx,
            dep_infos[0][ErlangDependencyInfo],
            dep_infos[1][ErlangDependencyInfo],
        )
    )

def _build_erlang_test(ctx: AnalysisContext, dep_info: ErlangDependencyInfo, binary_lib_dep_info: ErlangDependencyInfo) -> Promise:
    toolchain = get_toolchain(ctx)
    tools = toolchain.otp_binaries

    # prepare build environment
    build_environment = erlang_build.prepare_build_environment(dep_info, peek_private_includes = True)

    # Config files for ct
    config_files = [config_file[DefaultInfo].default_outputs[0] for config_file in ctx.attrs.config_files]

    cmd = cmd_args([trampoline[RunInfo] for trampoline in ctx.attrs._trampolines])
    cmd.add(tools.erl, default_test_args)

    app_folders = [dep[ErlangAppInfo].app_folder for dep in binary_lib_dep_info.dependencies.values() if not dep[ErlangAppInfo].virtual]
    cmd.add(cmd_args(app_folders, format = "{}/ebin", prepend = "-pa"))
    cmd.add("--")

    suite = ctx.attrs.suite
    suite_name = module_name(suite)
    hermetic_src_dir = ctx.actions.symlinked_dir(paths.join(erlang_build.utils.BUILD_DIR, "src"), {suite.basename: suite}, has_content_based_path = False)

    erlang_build.build_steps.generate_beam_artifacts(
        ctx,
        toolchain,
        build_environment,
        "tests",
        [suite],
        hermetic_src_dir,
    )

    beam = build_environment.beams["tests"][suite_name]
    ebin_dir = paths.dirname(beam.short_path)

    suite_data = paths.join(ebin_dir, suite_name + "_data")
    data_dir = _build_resource_dir(ctx, ctx.attrs.resources, suite_data)

    output_dir = link_output(ctx, beam, data_dir)
    test_info_file = _write_test_info_file(
        ctx = ctx,
        test_suite = suite_name,
        dep_info = dep_info,
        test_dir = output_dir,
        config_files = config_files,
        erl_cmd = toolchain.otp_binaries.erl,
        raw_target = str(ctx.label.raw_target()) if ctx.label else "",
        inner_trampolines = [trampoline[RunInfo].args for trampoline in ctx.attrs._inner_trampolines],
    )

    # Everything that comes after the `-extra` will be interpreted by the test_binary
    cmd.add("-extra")
    cmd.add(cmd_args(test_info_file, format = "--test-info-file={}"))

    default_info = _build_default_info(dep_info, output_dir)

    # NB. We can't use `quote="shell"` since we need $REPO_ROOT to be expanded by the shell.
    # So we wrap everything in extra double-quotes to protect from spaces in the path
    test_info_file_arg = cmd_args(test_info_file, format = '"<<\\"${REPO_ROOT}/{}\\">>"')

    additional_shell_args = cmd_args(
        cmd_args("-test_cli_lib", "test_info_file", test_info_file_arg, delimiter = " "),
        cmd_args("-eval", "test_cli_lib:start()", quote = "shell"),
        "-noshell",
    )

    run_info = erlang_shell.build_run_info(
        ctx,
        dep_info = dep_info,
        additional_code_path = cmd_args(output_dir),
        additional_shell_deps = [ctx.attrs._cli_lib],
        additional_args = additional_shell_args,
    )

    # Setup RE executors based on the `remote_execution` param.
    re_executors = get_re_executors_from_props(ctx)
    external_runner_info = ExternalRunnerTestInfo(
        type = "erlang_test",
        command = [cmd],
        env = ctx.attrs.env,
        labels = ctx.attrs.labels
        + [
            "tpx:supports-test-result-output-spec",
            "tpx:bundled-tests-have-per-test-details",
        ],
        contacts = ctx.attrs.contacts,
        run_from_project_root = True,
        use_project_relative_paths = True,
        default_executor = re_executors.default_executor,
        executor_overrides = re_executors.executor_overrides,
        supports_test_execution_caching = ctx.attrs.supports_test_execution_caching,
    )
    test_info = ErlangTestInfo(
        name = suite_name,
        dependencies = dep_info.dependencies,
        code_path_tset = ctx.actions.tset(
            CodePathTSet,
            value = CodePathEntry(dir = output_dir, ebin = False),
            children = [dep_info.code_path_tset],
        ),
        output_dir = output_dir,
    )

    return run_info.map(
        lambda run_info: [
            default_info,
            run_info,
            ErlangAppOrTestInfo(),
            external_runner_info,
            test_info,
        ]
    )

# Copied from erlang_application.
def _build_default_info(dep_info: ErlangDependencyInfo, output_dir: Artifact) -> Provider:
    """generate default_outputs and DefaultInfo provider"""

    # We depend on the code path of all dependencies to force them to be compiled
    # and emit errors when users compile just this one application
    return DefaultInfo(default_output = output_dir, other_outputs = [code_path_args(dep_info.code_path_tset)])

def _write_test_info_file(
    ctx: AnalysisContext,
    test_suite: str,
    dep_info: ErlangDependencyInfo,
    test_dir: Artifact,
    config_files: list[Artifact],
    erl_cmd: [cmd_args, Artifact],
    raw_target: str,
    inner_trampolines: list[cmd_args],
) -> WriteJsonCliArgs:
    tests_info = {
        "artifact_annotation_mfa": ctx.attrs._artifact_annotation_mfa,
        "common_app_env": ctx.attrs.common_app_env,
        "config_files": config_files,
        "ct_opts": ctx.attrs._ct_opts,
        "dependencies": code_path_args(dep_info.code_path_tset),
        "erl_cmd": erl_cmd,
        "extra_ct_hooks": ctx.attrs.extra_ct_hooks,
        "extra_flags": ctx.attrs.extra_erl_flags,
        "providers": ctx.attrs._providers,
        "raw_target": raw_target,
        "test_dir": test_dir,
        "test_suite": test_suite,
        "trampolines": inner_trampolines,
    }
    test_info_file = ctx.actions.declare_output("tests_info", has_content_based_path = False)
    return ctx.actions.write_json(test_info_file, tests_info, with_inputs = True)

def _build_resource_dir(ctx: AnalysisContext, resources: list, target_dir: str) -> [Artifact, None]:
    """build mapping for suite data directory

    generating the necessary mapping information for the suite data directory
    the resulting mapping can be used directly to symlink
    """
    if not resources:
        return None

    include_symlinks = {}
    for resource in resources:
        files = resource[DefaultInfo].default_outputs
        for file in files:
            if file.short_path in include_symlinks:
                fail("duplicate resource file: `{}`, defined in {} and {}".format(file.short_path, include_symlinks[file.short_path], file))
            else:
                include_symlinks[file.short_path] = file
    return ctx.actions.symlinked_dir(
        target_dir,
        include_symlinks,
        has_content_based_path = False,
    )

def link_output(ctx: AnalysisContext, beam: Artifact, data_dir: [Artifact, None]) -> Artifact:
    """Link the data_dirs and the test_suite beam in a single output folder."""
    link_spec = {
        beam.basename: beam,
        ctx.attrs.suite.basename: ctx.attrs.suite,
    }
    if data_dir:
        link_spec[data_dir.basename] = data_dir

    # Keep this name short. Common Test names each run's log directory after the last two
    # components of this path (`ct_run:get_name/1`, here `__<target>__.suite`), followed by the
    # suite, the group and, for a single-case run, the case name. That name must fit the 255-byte
    # file name limit, or CT cannot create the log directory and runs nothing.
    return ctx.actions.symlinked_dir("suite", link_spec, has_content_based_path = False)

def generate_file_map_target(suite: str, prefix: str | None, dir_name: str) -> str | None:
    suite_dir = paths.dirname(suite)
    suite_name = paths.basename(suite)
    suite_path = paths.join(suite_dir, dir_name)

    if is_target(suite):
        files = []
    else:
        files = glob([paths.join(suite_path, "**")])

    if len(files):
        if prefix != None:
            target_suffix = "{}_{}".format(prefix, suite_name)
        else:
            target_suffix = suite_name

        # generate target for data dir
        mapping_name = "{}-{}".format(dir_name, target_suffix)
        file_mapping(
            name = mapping_name,
            mapping = preserve_structure(
                path = suite_path,
            ),
        )
        return ":" + mapping_name

    return None

def is_target(suite: str) -> bool:
    if suite.startswith(":"):
        return True
    if suite.find("//") != -1:
        return True
    return False

erlang_test = prelude_rule(
    name = "erlang_test",
    impl = erlang_test_impl,
    docs = """
        The `erlang_test` ruls defines a test target for a single test suite. In most cases you
        want to define multiple suites in one go. The `erlang_tests` macro allows users to generate
        `erlang_test` targets for multiple test suites. Each suite `<name>_SUITE.erl` will have a
        generated hidden `erlang_test` target whose name is `<name>_SUITE`.

        Each `erlang_test` target implements tests using the Common Test library
        [OTP documentation](https://www.erlang.org/doc/man/common_test.html). They can,
        although **it is not recommended**, also act as dependencies of other tests. The
        default output of this rule is a "test_folder", consisting of the compiled test suite
        and the data directory.

        For each suite  `<name>_SUITE.erl`, if a data_dir `<name>_SUITE_data` is present along the suite,
        (as per [the data_dir naming scheme for ct](https://www.erlang.org/doc/apps/common_test/write_test_chapter.html#data-and-private-directories)),
        it will automatically adds the corresponding resource target to the generated test target of the suite.
        Resources will be placed in the [Data directory (data_dir)](https://www.erlang.org/doc/apps/common_test/write_test_chapter.html#data_priv_dir)
        of each of the suite.

        It allows the writer of the rule to add global configuration files and global default
        dependencies (e.g `meck`). These ones should be specified using global
        variables `erlang.erlang_tests_default_apps` and `erlang.erlang_tests_default_config`
        respectively.

        The `erlang_tests` macro forwards all attributes to the `erlang_test`. It defines some attributes
        that control how the targets get generated:
        - `srcs` ([source]): Set of files that the suites might depend on and that are not part of any specific application.
          A "meta" application having those files as sources will automatically be created, and included in the dependencies
          of the tests.

        One can call
        - `buck2 build //my_app:test_SUITE` to compile the test files together with its dependencies.
        - `buck2 test //my_app:other_test_SUITE` to run the test.
        - `buck2 run //my_app:other_test_SUITE` to open an interactive test shell, where tests can be run iteratively.

        buck2 test will rely on tpx to run the suite. To get access to tpx commands, add `--` after the
        target. For example:

        - `buck2 test //my_app:other_test_SUITE -- --help` will print the list of tpx available
        command line parameters.
        - `buck2 test //my_app:other_test_SUITE -- group.mycase` will only run those test cases
        that match the pattern `group.mycase`
    """,
    examples = """
        erlang_test(
            name = "unit_test_SUITE",
            suite = "unit_test_SUTIE.erl",
            deps = [":my_other_app"],
            contacts = ["author@email.com"],
        )

        erlang_tests(
            suites = ["test_SUITE.erl", "other_test_SUITE".erl],
            deps = [":my_app"],
            contacts = ["author@email.com"],
        )
    """,
    further = None,
    attrs = erlang_rule_attributes["erlang_test"],
)
