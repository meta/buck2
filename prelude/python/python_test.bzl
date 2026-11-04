# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:attrs_validators.bzl", "validation_common")
load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:attrs.bzl", "cxx_binary_base_attrs")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:native_common.bzl", "native_common")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:test_common.bzl", "test_common")
load("@prelude//python:attrs.bzl", "python_common", "python_executable_attrs", "typing_arg")
load("@prelude//python:compute_providers.bzl", "ExecutableType")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")
load("@prelude//utils:utils.bzl", "from_named_set", "value_or")
load(":interface.bzl", "EntryPointKind")
load(
    ":manifest.bzl",
    "get_srcs_from_manifest",
)
load(":python.bzl", "PythonLibraryInfo")
load(":python_binary.bzl", "python_executable")
load(":python_library.bzl", "py_attr_resources", "qualify_srcs")

def _write_test_modules_list(ctx: AnalysisContext, srcs: dict[str, Artifact]) -> (str, Artifact):
    """
    Generate a python source file with a list of all test modules.
    """
    name = "__test_modules__.py"
    contents = "TEST_MODULES = [\n"
    for dst in srcs:
        root, ext = paths.split_extension(dst)
        if ext != ".py":
            fail("test sources must end with .py")
        module = root.replace("/", ".")
        contents += '    "{}",\n'.format(module)
    contents += "]\n"
    return name, ctx.actions.write(name, contents, has_content_based_path = False)

def python_test_executable(ctx: AnalysisContext) -> list[Provider] | Promise:
    main_module = value_or(ctx.attrs.main_module, "__test_main__")

    srcs = qualify_srcs(ctx.label, ctx.attrs.base_module, from_named_set(ctx.attrs.srcs))
    if ctx.attrs.implicit_test_library != None:
        top_level_manifest = list(ctx.attrs.implicit_test_library[PythonLibraryInfo].manifests.traverse(ordering = "preorder"))[0]
        srcs.update(qualify_srcs(ctx.label, ctx.attrs.base_module, from_named_set(get_srcs_from_manifest(top_level_manifest.srcs))))

    test_modules_name, test_modules_path = _write_test_modules_list(ctx, srcs)
    srcs[test_modules_name] = test_modules_path

    # Add in default test runner.
    srcs["__test_main__.py"] = ctx.attrs._test_main

    resources_map, standalone_resources_map, outplace_resources_map = py_attr_resources(ctx)
    standalone_resources = qualify_srcs(ctx.label, ctx.attrs.base_module, standalone_resources_map)
    outplace_resources = qualify_srcs(ctx.label, ctx.attrs.base_module, outplace_resources_map)
    resources = qualify_srcs(ctx.label, ctx.attrs.base_module, resources_map)

    return python_executable(
        ctx,
        (EntryPointKind("module"), main_module),
        srcs,
        resources,
        standalone_resources,
        outplace_resources,
        compile = value_or(ctx.attrs.compile, False),
        allow_cache_upload = False,
        executable_type = ExecutableType("test"),
    )

def python_test_impl(ctx: AnalysisContext) -> list[Provider] | Promise:
    return python_test_executable(ctx)

python_test = prelude_rule(
    name = "python_test",
    impl = python_test_impl,
    docs = """
        A `python_test()` rule defines a set of `.py` files that contain tests to run via the [Python unit testing framework](https://docs.python.org/library/unittest.html).

         If your test requires static files you should specify these in
         the **resources** or **platform\\_resources** arguments.
         If you do not specify these files, they won't be available when your
         test runs.
    """,
    examples = """
        ```
        # A rule that includes a single .py file containing tests.
        python_test(
          name = 'fileutil_test',
          srcs = ['fileutil_tests.py'],
          deps = [
            ':fileutil',
          ],
        )

        # A rule that uses glob() to include all sources in the directory which the
        # rule is defined.  It also lists a resource file that gets packaged with
        # the sources in this rule.
        python_library(
          name = 'fileutil',
          srcs = glob(['fileutil/**/*.py']),
          resources = [
            'testdata.dat',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {k: attrs.default_only(v) for k, v in cxx_binary_base_attrs().items()}
        | buck.inject_test_env_arg()
        | python_common.srcs_arg()
        | python_common.resources_arg()
        | python_common.base_module_arg()
        | python_common.exclude_deps_from_merged_linking_arg()
        | {
            "main_module": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                The main module used to run the tests.
                 This parameter is normally not needed, as Buck will provide a default main
                 module that runs all tests. However, you can override this with your own
                 module to perform custom initialization or command line processing. Your
                 custom module can import the standard Buck test main
                 as `__test_main__`, and can invoke it's normal main function
                 as `__test_main__.main(sys.argv)`.
            """,
            ),
        }
        | {
            "env": attrs.dict(
                key = attrs.string(),
                value = attrs.arg(),
                sorted = False,
                default = {},
                doc = """
                A map of environment names and values to set when running the test.

                 It is also possible to expand references to other rules within the **values** of
                 these environment variables, using builtin `string parameter macros`
                :

                `$(location //path/to:target)`
                Expands to the location of the output of the build rule. This
                 means that you can refer to these without needing to be aware of how
                 Buck is storing data on the disk mid-build.
            """,
            ),
        }
        | python_common.deps_arg()
        | python_common.version_selections_arg()
        | buck.test_rule_timeout_ms()
        | python_common.package_style_arg()
        | python_common.preload_deps_arg()
        | python_common.linker_flags_arg()
        | python_common.deduplicate_merged_link_roots()
        | python_common.executable_deps_arg()
        | native_common.link_group_deps()
        | native_common.link_group_public_deps_label()
        | native_common.transformation_spec_arg()
        | {
            "additional_coverage_targets": attrs.list(attrs.dep(), default = []),
            "build_args": attrs.list(attrs.arg(), default = []),
            "compile": attrs.option(attrs.bool(), default = None),
            "dummy_omnibus": attrs.option(attrs.dep(), default = None),
            "extension": attrs.option(attrs.string(), default = None),
            "needed_coverage": attrs.list(attrs.tuple(attrs.int(), attrs.dep(), attrs.option(attrs.string())), default = []),
            "prefer_stripped_native_objects": attrs.bool(default = False),
            "repl_main": attrs.option(attrs.string(), default = None),
            "repl_only_deps": attrs.list(attrs.dep(), default = []),
            "runner": attrs.option(attrs.dep(), default = None),
            "specs": attrs.option(attrs.arg(json = True), default = None),
            "versioned_resources": attrs.option(attrs.versioned(attrs.named_set(attrs.source(), sorted = True)), default = None),
            "versioned_srcs": attrs.option(attrs.versioned(attrs.named_set(attrs.source(), sorted = True)), default = None),
            "zip_safe": attrs.option(attrs.bool(), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | typing_arg()
        | test_common.attributes()
        | python_executable_attrs()
        | {
            "implicit_test_library": attrs.option(attrs.dep(providers = [PythonLibraryInfo]), default = None),
            "safer_lazy_imports": attrs.bool(default = False),  # TODO(T240038931) When enabling lazy imports by default, remove this line
            "supports_test_execution_caching": attrs.bool(default = True),
            "_test_main": attrs.source(default = "prelude//python/tools:__test_main__.py"),
        }
        | re_test_common.test_args()
        | validation_common.attrs_validators_arg()
    ),
    cfg = constraint_overrides.python_transition,
)
