# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:attrs.bzl", "BUILD_INFO_ATTR", "cxx_common")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:test_common.bzl", "test_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//go/transitions:defs.bzl", "build_tags_attr", "cgo_enabled_attr", "coverage_mode_attr", "go_test_transition")
load(
    "@prelude//linking:link_info.bzl",
    "LinkStyle",
)
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")
load(
    "@prelude//tests:re_utils.bzl",
    "get_re_executors_from_props",
)
load(
    "@prelude//utils:utils.bzl",
    "from_named_set",
    "map_val",
    "value_or",
)
load(":attrs.bzl", "BuildMode", "go_common")
load(":cgo_builder.bzl", "get_cgo_build_context")
load(":compile.bzl", "GoTestInfo")
load(":coverage.bzl", "GoCoverageMode")
load(":link.bzl", "GoBuildMode", "get_inherited_link_pkgs", "link")
load(":package_builder.bzl", "GoBuildConfig", "GoSourceInputs", "declare_package_build")
load(":packages.bzl", "go_attr_pkg_name")
load(":toolchain.bzl", "GoToolchainInfo", "evaluate_cgo_enabled")

def _gen_test_main(
    ctx: AnalysisContext,
    pkg_import_path: str,
    coverage_mode: [GoCoverageMode, None],
    cover_packages: list[str],  # packages those are included for coverage
    test_go_files_argsfile: Artifact,
) -> Artifact:
    """
    Generate a `main.go` which calls tests from the given sources.
    """
    output = ctx.actions.declare_output("main.go", has_content_based_path = True)
    cmd = []
    cmd.append(ctx.attrs._testmaingen[RunInfo])

    # if ctx.attrs.coverage_mode:
    cmd.append(cmd_args(output.as_output(), format = "--output={}"))
    cmd.append(cmd_args(pkg_import_path, format = "--import-path={}"))
    if coverage_mode != None:
        cmd.extend(["--cover-mode", coverage_mode.value])
    if cover_packages:
        cover_pkgs_argsfile = ctx.actions.declare_output("cover_pkgs_argsfile", has_content_based_path = True)
        ctx.actions.write(cover_pkgs_argsfile, [["--cover-pkgs", pkg] for pkg in cover_packages])
        cmd.append(cmd_args(cover_pkgs_argsfile, format = "@{}"))
    cmd.append(cmd_args(test_go_files_argsfile, format = "@{}"))
    ctx.actions.run(cmd_args(cmd), category = "go_test_main_gen", allow_cache_upload = ctx.attrs._go_toolchain[GoToolchainInfo].allow_cache_upload)
    return output

def is_subpackage_of(other_pkg_import_path: str, pkg_import_path: str) -> bool:
    return pkg_import_path == other_pkg_import_path or other_pkg_import_path.startswith(pkg_import_path + "/")

def go_test_impl(ctx: AnalysisContext) -> list[Provider]:
    pkg_import_path = go_attr_pkg_name(ctx)
    cgo_enabled = evaluate_cgo_enabled(ctx.attrs._cgo_enabled)

    deps = ctx.attrs.deps
    srcs = ctx.attrs.srcs
    coverage_enabled = ctx.attrs.coverage_enabled

    # Copy the srcs, deps and pkg_import_path from the target library when set. The
    # library code gets compiled together with the tests.
    if ctx.attrs.target_under_test:
        lib = ctx.attrs.target_under_test[GoTestInfo]
        srcs += lib.srcs
        deps += lib.deps

        # TODO: should we assert that pkg_import_path != None here?
        pkg_import_path = lib.pkg_import_path
        coverage_enabled = coverage_enabled or lib.coverage_enabled

    # If coverage is enabled for this test, we need to preprocess the sources
    # with the Go cover tool.
    coverage_mode = GoCoverageMode(ctx.attrs._coverage_mode) if ctx.attrs._coverage_mode else None
    cgo_build_context = get_cgo_build_context(ctx)
    pkgs = {}

    # Compile all tests into a package.
    tests, tests_pkg_info, test_go_files_argsfile = declare_package_build(
        ctx = ctx,
        pkg_import_path = pkg_import_path,
        main = False,
        sources = GoSourceInputs(
            srcs = srcs,
            embed_srcs = from_named_set(ctx.attrs.embed_srcs),
            package_root = ctx.attrs.package_root,
        ),
        cgo_build_context = cgo_build_context,
        config = GoBuildConfig(
            compiler_flags = ctx.attrs.compiler_flags,
            build_tags = ctx.attrs._build_tags,
            coverage_enabled = coverage_enabled,
            coverage_mode = coverage_mode,
            with_tests = True,
            cgo_enabled = cgo_enabled,
        ),
        pkgs = pkgs,
        deps = deps,
    )

    cover_packages = []

    # Cover the test package itself
    if tests.coverage_instrumented:
        cover_packages.append(pkg_import_path)

    # Get all packages that are linked to the test (i.e. the entire dependency tree)
    for import_path, pkg in get_inherited_link_pkgs(deps).items():
        if pkg.coverage_instrumented:
            # Cover dependencies with coverage instrumented
            cover_packages.append(import_path)

    pkgs[pkg_import_path] = tests

    # Generate a 'main.go' file (test runner) which runs the actual tests from the package above.
    # Build the it as a separate package (<foo>.test) - which imports and invokes the test package.
    gen_main = _gen_test_main(ctx, pkg_import_path, coverage_mode, cover_packages, test_go_files_argsfile)
    main, _, _ = declare_package_build(
        ctx = ctx,
        pkg_import_path = pkg_import_path + ".test",
        main = True,
        sources = GoSourceInputs(
            srcs = [gen_main],
            package_root = "",
        ),
        cgo_build_context = None,
        config = GoBuildConfig(
            cgo_enabled = cgo_enabled,
        ),
        pkgs = pkgs,
        cgo_gen_dir_name = "cgo_gen_test_main",
    )

    # Link the above into a Go binary.
    (bin, runtime_files, external_debug_info) = link(
        ctx = ctx,
        main = main,
        cgo_enabled = cgo_enabled,
        pkgs = pkgs,
        deps = deps,
        link_style = value_or(map_val(LinkStyle, ctx.attrs.link_style), LinkStyle("static")),
        build_mode = GoBuildMode(value_or(ctx.attrs.build_mode, "exe")),
        linker_flags = ctx.attrs.linker_flags,
        external_linker_flags = ctx.attrs.external_linker_flags,
    )

    # As per v1, copy in resources next to binary.
    copied_resources = []
    for resource in ctx.attrs.resources:
        copied_resources.append(ctx.actions.copy_file(resource.short_path, resource, has_content_based_path = False))

    run_cmd = cmd_args(bin, hidden = [runtime_files, external_debug_info] + copied_resources)

    # Setup RE executors based on the `remote_execution` param.
    re_executors = get_re_executors_from_props(ctx)

    # Emit TPX_LIST_TESTS_COMMAND so TPX can enumerate tests via AST parsing
    # of the *_test.go sources instead of running the compiled binary with
    # -test.list. Eliminates cgo and TestMain startup costs from listing.
    # The lister mirrors `go test -list <regex>`
    env = dict(ctx.attrs.env)
    env["TPX_LIST_TESTS_COMMAND"] = cmd_args(
        [ctx.attrs._list_tests[RunInfo]],
        "-match=^Test.*",
        cmd_args(test_go_files_argsfile, format = "@{}"),
    )

    return inject_test_run_info(
        ctx,
        ExternalRunnerTestInfo(
            type = "go",
            command = [run_cmd],
            env = env,
            labels = ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            default_executor = re_executors.default_executor,
            executor_overrides = re_executors.executor_overrides,
            run_from_project_root = True,
            use_project_relative_paths = re_executors.use_project_relative_paths,
            supports_test_execution_caching = ctx.attrs.supports_test_execution_caching,
        ),
    ) + [
        DefaultInfo(
            default_output = bin,
            other_outputs = [gen_main] + runtime_files + external_debug_info + copied_resources,
        ),
        tests_pkg_info,
    ]

go_test = prelude_rule(
    name = "go_test",
    impl = go_test_impl,
    docs = """
        A `go_test()` rule builds a native binary from the
         specified Go source and resource files—and a generated main file. It's
         similar to the `go test` command.

         If your test requires static files you should specify these in
         the **resources** argument. If you do not specify these
         files, they won't be available when your test runs.
    """,
    examples = """
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/features/go/testdata).

        ```
        go_library(
          name='greeting',
          srcs=[
            'greeting.go',
          ],
          deps=[
            ':join',
          ],
        )

        go_test(
          name='greeting-test',
          srcs=[
            'greeting_ext_test.go',
          ],
          deps=[
            ':greeting'
          ],
        )

        go_test(
          name='greeting-internal-test',
          package_name='greeting',
          srcs=[
            'greeting.go',
            'greeting_test.go',
          ],
          deps=[
            ':join',
          ],
        )

        # Or

        go_test(
          name='greeting-better-internal-test',
          srcs=['greeting_test.go'],
          target_under_test=':greeting',
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        buck.inject_test_env_arg()
        | go_common.srcs_arg()
        | {
            "package_name": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Sets the full name of the test package being compiled. This defaults to the path from the buck
                 root with "\\_test" appended. (e.g. given a ./.buckconfig, a rule in ./a/b/BUCK defaults to package "a/b\\_test")

                 Note: if you want to test packages internally (i.e. same package name), use the `target_under_test`
                 parameter instead of setting `package_name` to include the tested source files.
            """,
            ),
            "target_under_test": attrs.option(
                attrs.dep(),
                default = None,
                doc = """
                Specify the target_under_test that this internal test is testing. This will copy the `srcs`,
                 `package_name` and `deps` from the target specified so you don't have
                 to duplicate them.
            """,
            ),
        }
        | go_common.deps_arg()
        | go_common.link_style_arg()
        | go_common.coverage_enabled_arg()
        | go_common.link_mode_arg()
        | {
            "build_mode": attrs.option(
                attrs.enum(BuildMode),
                doc = """
                Determines the build mode (equivalent of `-buildmode`). Can be
                 one of the following values: `exe`, `pie`.`
            """,
                default = None,
            ),
        }
        | go_common.compiler_flags_arg()
        | go_common.assembler_flags_arg()
        | go_common.linker_flags_arg()
        | go_common.external_linker_flags_arg()
        | go_common.embed_srcs_arg()
        | go_common.package_root_arg()
        | go_common.cgo_enabled_arg()
        | go_common.build_tags_arg()
        | cxx_common.headers_arg()
        | cxx_common.header_namespace_arg()
        | go_common.cxx_preprocessor_flags_arg()
        | go_common.cxx_compiler_flags_arg()
        | buck.test_label_arg()
        | buck.test_rule_timeout_ms()
        | {
            "env": attrs.dict(
                key = attrs.string(),
                value = attrs.arg(),
                sorted = False,
                default = {},
                doc = """
                A map of environment variables and values to set when running the test.
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
        | {
            "platform": attrs.option(attrs.string(), default = None),
            "runner": attrs.option(attrs.dep(), default = None),
            "specs": attrs.option(attrs.arg(json = True), default = None),
            "supports_test_execution_caching": attrs.bool(default = True),
        }
        | buck.licenses_arg()
        | buck.contacts_arg()
        | re_test_common.test_args()
        | test_common.attributes()
        | {
            "coverage_mode": attrs.option(
                attrs.enum(GoCoverageMode.values()),
                default = None,
                doc = """
                Test coverage functionality will be included in the executable.
            """,
            ),
            "resources": attrs.list(
                attrs.source(allow_directory = True),
                default = [],
                doc = """
                Static files that are symlinked into the working directory of the
                 test. You can access these files in your test by opening them using
                 relative paths, such as `ioutil.ReadFile("testdata/input")`.
            """,
            ),
            "_build_info": BUILD_INFO_ATTR,
            "_build_tags": build_tags_attr,
            "_cgo_enabled": cgo_enabled_attr,
            "_coverage_mode": coverage_mode_attr,
            "_cxx_toolchain": toolchains_common.cxx(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_go_stdlib": attrs.default_only(attrs.dep(default = "prelude//go/tools:stdlib")),
            "_go_toolchain": toolchains_common.go(),
            "_list_tests": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//go/tools:list_tests")),
            "_testmaingen": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//go/tools:testmaingen")),
        }
    ),
    cfg = go_test_transition,
)
