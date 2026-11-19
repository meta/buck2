# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:attrs_validators.bzl", "validation_common")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//decls:common.bzl", "TestType", "buck")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:test_common.bzl", "test_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(
    "@prelude//java:class_to_srcs.bzl",
    "JavaClassToSourceMapInfo",  # @unused Used as a type
    "merge_class_to_source_map_from_jar",
)
load("@prelude//java:java_library.bzl", "build_java_library", "jvm_target_stats")
load("@prelude//java:java_providers.bzl", "JavaLibraryInfo", "JavaPackagingInfo", "get_all_java_packaging_deps_tset")
load("@prelude//java:java_toolchain.bzl", "JavaTestToolchainInfo", "JavaToolchainInfo")
load("@prelude//java/utils:java_more_utils.bzl", "get_path_separator_for_exec_os")
load("@prelude//jvm:attrs.bzl", "jvm_common")
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "create_shlib_symlink_tree",
    "merge_shared_libraries",
    "traverse_shared_library_info",
)
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")
load(
    "@prelude//tests:re_utils.bzl",
    "get_re_executors_from_props",
)
load("@prelude//tests:test_listing.bzl", "TestListingInfo")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")
load("@prelude//utils:argfile.bzl", "at_argfile")
load("@prelude//utils:expect.bzl", "expect")

def java_test_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs._build_only_native_code:
        return [DefaultInfo()]

    target_stats_providers, target_stats_subtargets = jvm_target_stats(ctx)
    java_providers = build_java_library(
        ctx,
        ctx.attrs.srcs,
        extra_sub_targets = target_stats_subtargets,
    )
    external_runner_test_info = build_junit_test(ctx, java_providers.java_library_info, java_providers.java_packaging_info, java_providers.class_to_src_map)

    providers = [
        java_providers.java_library_intellij_info,
        java_providers.java_library_info,
        java_providers.java_packaging_info,
        java_providers.template_placeholder_info,
        java_providers.default_info,
        java_providers.class_to_src_map,
    ]
    if java_providers.validation_info:
        providers.append(java_providers.validation_info)
    providers.extend(target_stats_providers)

    return inject_test_run_info(ctx, external_runner_test_info) + providers

def build_junit_test(
    ctx: AnalysisContext,
    tests_java_library_info: JavaLibraryInfo,
    tests_java_packaging_info: JavaPackagingInfo,
    tests_class_to_source_info: [JavaClassToSourceMapInfo, None] = None,
    extra_cmds: list = [],
    extra_classpath_entries: list[Artifact] = [],
) -> ExternalRunnerTestInfo:
    java_test_toolchain = ctx.attrs._java_test_toolchain[JavaTestToolchainInfo]
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]

    java = ctx.attrs.java[RunInfo] if ctx.attrs.java else java_toolchain.java_for_tests

    cmd = [java] + extra_cmds + ctx.attrs.vm_args + ["-XX:-MaxFDLimit"]
    if java_test_toolchain.jvm_args:
        cmd.extend(java_test_toolchain.jvm_args)

    cmd.append(cmd_args(ctx.attrs.java_agents, format = "-javaagent:{}"))

    classpath = (
        [
            java_test_toolchain.test_runner_library_jar,
        ]
        + [
            get_all_java_packaging_deps_tset(ctx, java_packaging_infos = [tests_java_packaging_info]).project_as_args("full_jar_args", ordering = "bfs"),
        ]
        + extra_classpath_entries
    )

    if ctx.attrs.unbundled_resources_root:
        classpath.append(ctx.attrs.unbundled_resources_root)

    labels = ctx.attrs.labels or []

    # Setup RE executors based on the `remote_execution` param.
    re_executors = get_re_executors_from_props(ctx)

    # We implicitly make the target run from the project root if remote
    # execution options were specified.
    run_from_cell_root = "buck2_run_from_cell_root" in labels

    uses_java8 = "run_with_java8" in labels

    relative_to = {"relative_to": ctx.label.cell_root} if run_from_cell_root else {}

    if uses_java8:
        # Java 8 does not support using argfiles, and these tests can have huge classpaths so we need another
        # mechanism to write the classpath to a file.
        # We add "FileClassPathRunner" to the classpath, and then write a line-separated classpath file which we pass
        # to the "FileClassPathRunner" as a system variable. The "FileClassPathRunner" then loads all the jars
        # from that file onto the classpath, and delegates running the test to the junit test runner.
        cmd.extend(["-classpath", cmd_args(java_test_toolchain.test_runner_library_jar)])
        classpath_args = cmd_args(cmd_args(classpath), **relative_to)
        classpath_args_file = ctx.actions.write("classpath_args_file", classpath_args, has_content_based_path = False)
        cmd.append(
            cmd_args(
                classpath_args_file,
                format = "-Dbuck.classpath_file={}",
                hidden = classpath_args,
            )
        )
    else:
        # Java 9+ supports argfiles, so just write the classpath to an argsfile. "FileClassPathRunner" will delegate
        # immediately to the junit test runner.
        classpath_args = cmd_args("-classpath", cmd_args(classpath, delimiter = get_path_separator_for_exec_os(ctx)), **relative_to)
        cmd.append(at_argfile(actions = ctx.actions, name = "classpath_args_file", args = classpath_args))

    if ctx.attrs.test_type == "junit5":
        cmd.extend(java_test_toolchain.junit5_test_runner_main_class_args)
    elif ctx.attrs.test_type == "testng":
        cmd.extend(java_test_toolchain.testng_test_runner_main_class_args)
    else:
        cmd.extend(java_test_toolchain.junit_test_runner_main_class_args)

    if ctx.attrs.test_case_timeout_ms:
        cmd.extend(["--default-test-timeout", str(ctx.attrs.test_case_timeout_ms)])

    if ctx.attrs.test_class_names_file and getattr(ctx.attrs, "discover_all_test_classes", False):
        fail("Cannot set both test_class_names_file and discover_all_test_classes")

    if ctx.attrs.test_class_names_file:
        class_names = ctx.attrs.test_class_names_file
    else:
        expect(tests_java_library_info.library_output != None, "Built test library has no output, likely due to missing srcs")
        class_names = ctx.actions.declare_output("class_names", has_content_based_path = False)
        discover_all = getattr(ctx.attrs, "discover_all_test_classes", False)
        list_class_names_args = [
            java_test_toolchain.list_class_names[RunInfo],
            "--jar",
            tests_java_library_info.library_output.full_library,
            "--sources",
            ctx.actions.write("sources.txt", [] if discover_all else ctx.attrs.srcs, has_content_based_path = False),
            "--output",
            class_names.as_output(),
        ]
        if discover_all:
            list_class_names_args.append("--discover-all")
        list_class_names_cmd = cmd_args(list_class_names_args, hidden = ctx.attrs.srcs)
        ctx.actions.run(list_class_names_cmd, category = "list_class_names")

    cmd.extend(["--test-class-names-file", class_names])

    native_libs_env = _get_native_libs_env(ctx)
    env = {}
    for d in [ctx.attrs.env, native_libs_env]:
        for key, value in d.items():
            if key in env:
                fail("Duplicate key for java_test env: '{}'".format(key))
            env[key] = value

    if tests_class_to_source_info != None:
        transitive_class_to_src_map = merge_class_to_source_map_from_jar(
            actions = ctx.actions,
            name = ctx.label.name + ".transitive_class_to_src.json",
            java_toolchain = java_toolchain,
            relative_to = ctx.label.cell_root if run_from_cell_root else None,
            deps = [tests_class_to_source_info],
        )
        if run_from_cell_root:
            transitive_class_to_src_map = cmd_args(transitive_class_to_src_map, relative_to = ctx.label.cell_root)
        env["JACOCO_CLASSNAME_SOURCE_MAP"] = transitive_class_to_src_map

    list_tests_info = ctx.attrs._java_test_toolchain[TestListingInfo]
    list_tests = list_tests_info.list_tests
    if list_tests != None and "tpx:supports_static_listing=true" in ctx.attrs.labels and "tpx:supports_static_listing=false" not in ctx.attrs.labels:
        list_tests_command = cmd_args([
            list_tests[RunInfo],
            "list-tests",
            "--sources-file",
            ctx.actions.write("source_files.txt", ctx.attrs.srcs, with_inputs = True, has_content_based_path = False),
        ])
        env["TPX_LIST_TESTS_COMMAND"] = list_tests_command

    test_info_type = getattr(ctx.attrs, "test_info_type", None) or "junit"
    test_info = ExternalRunnerTestInfo(
        type = test_info_type,
        command = cmd,
        env = env,
        labels = ctx.attrs.labels,
        contacts = ctx.attrs.contacts,
        run_from_project_root = not run_from_cell_root,
        use_project_relative_paths = not run_from_cell_root,
        default_executor = re_executors.default_executor,
        executor_overrides = re_executors.executor_overrides,
        supports_test_execution_caching = ctx.attrs.supports_test_execution_caching,
    )
    return test_info

def _get_native_libs_env(ctx: AnalysisContext) -> dict:
    if not ctx.attrs.use_cxx_libraries:
        return {}

    deps_to_search = ctx.attrs.cxx_library_allowlist or ctx.attrs.deps
    shared_library_infos = filter(None, [x.get(SharedLibraryInfo) for x in deps_to_search])

    shared_library_info = merge_shared_libraries(
        ctx.actions,
        deps = shared_library_infos,
    )

    cxx_library_symlink_tree = create_shlib_symlink_tree(
        actions = ctx.actions,
        out = "cxx_library_symlink_tree",
        shared_libs = traverse_shared_library_info(shared_library_info, transformation_provider = None),
    )

    return {"BUCK_LD_SYMLINK_TREE": cxx_library_symlink_tree}

java_test = prelude_rule(
    name = "java_test",
    impl = java_test_impl,
    docs = """
        A `java_test()` rule is used to define a set of
         `.java` files that contain tests to run via JUnit.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        buck.inject_test_env_arg()
        | {
            "resources": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Same as `java_library()`.
            """,
            ),
            "srcs": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Like `java_library()`,
                 all of the `.java` files specified by the
                 `srcs` argument will be compiled when this rule is
                 built. In addition, all of the corresponding `.class` files that are built by this rule will be passed as arguments to
                 JUnit when this rule is run as a test. `.class` files
                 that are passed to JUnit that do not have any methods annotated with
                 `@Test` are considered failed tests, so make sure that
                 only test case classes are specified as `srcs`. This is
                 frequently done by specifying `srcs` as
                 `glob(['**/*Test.java'])`.
            """,
            ),
        }
        | buck.test_label_arg()
        | {
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Same as `java_library()`.
                 // org.junit.rules.Timeout was not introduced until 4.7.
                 Must include JUnit (version 4.7 or later) as a dependency for JUnit tests.
                 Must include TestNG (version 6.2 or later) and hamcrest as a dependencies for TestNG tests.
            """,
            ),
            "source": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Java language level for compiling. Corresponds to the
                 `-source` argument for `javac`.
            """,
            ),
            "target": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Bytecode target level for compiling. Corresponds to the
                 `-target` argument for `javac`.
            """,
            ),
            "test_type": attrs.option(
                attrs.enum(TestType),
                default = None,
                doc = """
                Specifies which test framework to use.
                 The currently supported options are 'junit' and 'testng'.
            """,
            ),
        }
        | buck.run_test_separately_arg(
            run_test_separately_type = attrs.bool(
                default = False,
                doc = """
            If set to True, the test(s) in this rule are run separately from
             all other tests. This is useful for integration tests which
             access a physical device or other limited resource. If unset,
             the test(s) in this rule run in parallel with all other tests.
        """,
            )
        )
        | re_test_common.test_args()
        | buck.test_rule_timeout_ms()
        | {
            "cxx_library_allowlist": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                 List of cxx_library targets to build, if use_cxx_libraries is true.
                 This can be useful if some dependencies are Android-only and won't build for the test host platform.
            """,
            ),
            "use_cxx_libraries": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                Whether or not to build and link against `cxx_library()` dependencies when testing.
            """,
            ),
            "vm_args": attrs.list(
                attrs.arg(),
                default = [],
                doc = """
                Runtime arguments to the JVM running the tests.
            """,
            ),
        }
        | jvm_common.test_env()
        | jvm_common.abi_generation_mode()
        | {
            "deps_query": attrs.option(attrs.query(), default = None),
            "discover_all_test_classes": attrs.bool(default = False),
            "exported_deps": attrs.list(attrs.dep(), default = []),
            "exported_provided_deps": attrs.list(attrs.dep(), default = []),
            "extra_arguments": attrs.list(attrs.string(), default = []),
            "jar_postprocessor": attrs.option(attrs.exec_dep(), default = None),
            "java": attrs.option(attrs.dep(), default = None),
            "java_agents": attrs.list(attrs.source(), default = []),
            "java_version": attrs.option(attrs.string(), default = None),
            "manifest_file": attrs.option(attrs.source(), default = None),
            "maven_coords": attrs.option(attrs.string(), default = None),
            "proguard_config": attrs.option(attrs.source(), default = None),
            "provided_deps": attrs.list(attrs.dep(), default = []),
            "remove_classes": attrs.list(attrs.regex(), default = []),
            "required_for_source_only_abi": attrs.bool(default = False),
            "resources_root": attrs.option(attrs.string(), default = None),
            "runner": attrs.option(attrs.dep(), default = None),
            "runtime_deps": attrs.list(attrs.dep(), default = []),
            "source_only_abi_deps": attrs.list(attrs.dep(), default = []),
            "supports_test_execution_caching": attrs.bool(default = True),
            "test_case_timeout_ms": attrs.option(attrs.int(), default = None),
            "test_class_names_file": attrs.option(attrs.source(), default = None),
            "unbundled_resources_root": attrs.option(attrs.source(allow_directory = True), default = None),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_is_building_android_binary": attrs.default_only(attrs.bool(default = False)),
            "_java_test_toolchain": toolchains_common.java_test(),
            "_java_toolchain": toolchains_common.java(),
        }
        | constraint_overrides.attributes
        | buck.licenses_arg()
        | buck.contacts_arg()
        | validation_common.attrs_validators_arg()
    )
    | jvm_common.annotation_processors()
    | jvm_common.plugins()
    | jvm_common.javac()
    | test_common.attributes()
    | jvm_common.classic_java_content_based_paths(),
    cfg = constraint_overrides.transition,
)
