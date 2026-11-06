# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//android:configuration.bzl", "is_building_android_binary_attr")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_library.bzl", "compile_to_jar")
load(
    "@prelude//java:java_providers.bzl",
    "JavaCompileOutputs",  # @unused Used as a type
    "JavaLibraryInfo",
    "JavaPackagingDepTSet",
    "JavaPackagingInfo",
    "create_java_packaging_dep",
    "single_library_compiling_deps",
)
load(":android_providers.bzl", "AndroidBuildConfigInfo", "BuildConfigField", "merge_android_packageable_info")

def android_build_config_impl(ctx: AnalysisContext) -> list[Provider]:
    providers = []

    default_build_config_fields = get_build_config_fields(ctx.attrs.values)
    android_build_config_info = AndroidBuildConfigInfo(
        package = ctx.attrs.package, build_config_fields = default_build_config_fields, values_file = ctx.attrs.values_file
    )
    providers.append(android_build_config_info)
    providers.append(merge_android_packageable_info(ctx.label, ctx.actions, deps = [], build_config_info = android_build_config_info))

    default_values_files = [write_build_config_defaults(ctx, ctx.attrs.package, default_build_config_fields)]
    if ctx.attrs.values_file:
        default_values_files.append(ctx.attrs.values_file)

    build_config_dot_java_library, java_packaging_info, build_config_dot_java = generate_android_build_config(
        ctx,
        ctx.attrs.name,
        ctx.attrs.package,
        use_constant_expressions = False,
        default_values_files = default_values_files,
        values_files = [],
    )

    providers.append(java_packaging_info)
    providers.append(build_config_dot_java_library)

    providers.append(
        DefaultInfo(
            default_output = build_config_dot_java_library.library_output.full_library,
            sub_targets = {
                "build_config_dot_java": [DefaultInfo(default_output = build_config_dot_java)],
            },
        ),
    )
    return providers

def generate_android_build_config(
    ctx: AnalysisContext,
    source: str,
    java_package: str,
    use_constant_expressions: bool,
    default_values_files: list[Artifact],
    values_files: list[Artifact],
) -> (JavaLibraryInfo, JavaPackagingInfo, Artifact):
    build_config_dot_java = _generate_build_config_dot_java(ctx, source, java_package, use_constant_expressions, default_values_files, values_files)

    compiled_build_config_dot_java = _compile_and_package_build_config_dot_java(ctx, java_package, build_config_dot_java)
    library_output = compiled_build_config_dot_java.classpath_entry

    packaging_deps_kwargs = {
        "value": create_java_packaging_dep(ctx, library_output.full_library, compiler_depth = library_output.full_library_compiler_depth),
    }
    packaging_deps = ctx.actions.tset(JavaPackagingDepTSet, **packaging_deps_kwargs)
    return (
        JavaLibraryInfo(
            compiling_deps = single_library_compiling_deps(ctx.actions, library_output),
            library_output = library_output,
            output_for_classpath_macro = library_output.full_library,
        ),
        JavaPackagingInfo(
            packaging_deps = packaging_deps,
        ),
        build_config_dot_java,
    )

def _generate_build_config_dot_java(
    ctx: AnalysisContext,
    source: str,
    java_package: str,
    use_constant_expressions: bool,
    default_values_files: list[Artifact],
    values_files: list[Artifact],
) -> Artifact:
    generate_build_config_cmd = cmd_args(ctx.attrs._android_toolchain[AndroidToolchainInfo].generate_build_config[RunInfo])
    generate_build_config_cmd.add([
        "--source",
        source,
        "--java-package",
        java_package,
        "--use-constant-expressions",
        str(use_constant_expressions),
    ])

    for values_file in default_values_files:
        generate_build_config_cmd.add(["--default-values-file", values_file])
    for values_file in values_files:
        generate_build_config_cmd.add(["--values-file", values_file])

    build_config_dot_java = ctx.actions.declare_output(
        _get_output_name(java_package, "BuildConfig.java"),
        has_content_based_path = True,
    )
    generate_build_config_cmd.add(["--output", build_config_dot_java.as_output()])

    ctx.actions.run(
        generate_build_config_cmd,
        category = "android_generate_build_config",
        identifier = java_package,
    )

    return build_config_dot_java

def write_build_config_defaults(ctx: AnalysisContext, java_package: str, fields: list[BuildConfigField]) -> Artifact:
    return ctx.actions.write(
        _get_output_name(java_package, "default_values"),
        ["{} {} = {}".format(field.type, field.name, field.value) for field in fields],
        has_content_based_path = True,
    )

def _compile_and_package_build_config_dot_java(ctx: AnalysisContext, java_package: str, build_config_dot_java: Artifact) -> JavaCompileOutputs:
    return compile_to_jar(
        ctx,
        actions_identifier = "build_config_{}".format(java_package.replace(".", "_")),
        srcs = [build_config_dot_java],
    )

def get_build_config_fields(lines: list[str]) -> list[BuildConfigField]:
    return [_get_build_config_field(line) for line in lines]

def _get_build_config_field(line: str) -> BuildConfigField:
    type_and_name, value = [x.strip() for x in line.split("=", 1)]
    field_type, name = type_and_name.split()
    return BuildConfigField(type = field_type, name = name, value = value)

def _get_output_name(java_package: str, output_filename: str) -> str:
    return "android_build_config/{}/{}".format(java_package.replace(".", "_"), output_filename)

android_build_config = prelude_rule(
    name = "android_build_config",
    impl = android_build_config_impl,
    docs = """
        An `android_build_config()` rule is used to generate
        a `BuildConfig` class with global configuration variables
        that other `android_library()` rules can compile against.
        Currently, the only variable exposed by `BuildConfig` is
        a global `boolean` named `DEBUG`, much like
        the `BuildConfig.java` generated by the official Android
        build tools based on Gradle.

        The fields in the generated `BuildConfig` class will
        be non-constant-expressions (see [JLS 15.28](http://docs.oracle.com/javase/specs/jls/se7/html/jls-15.html#jls-15.28)).
        However, if `BuildConfig` is packaged into an APK, it will
        be replaced with a new version where:
        * The fields will be set to literal values (i.e., constant expressions).
         * The `boolean BuildConfig.DEBUG` field will correspond to
        that of the `package_type` argument to the `android_binary()` rule
        that is packaging it.

        This transformation is done before ProGuard is applied (if applicable), so
        that it can propagate constants from `BuildConfig` and eliminate
        dead code.
    """,
    examples = """
        Here is an example of an `android_build_config()` rule that
        is transitively included by both *debug* and *release* versions
        of an `android_binary()` rule. The value
        of `com.example.pkg.BuildConfig.DEBUG` will be different in each APK
        even though they both transitively depend on the same `:build_config` rule.

        ```
        android_build_config(
          name = 'build_config',
          package = 'com.example.pkg',
          values = [
            'String COMMIT_ID = "0000000000000000000000000000000000000000"',
          ],
        )

        # The .java files in this library may contain references to the boolean
        # com.example.pkg.BuildConfig.DEBUG because :build_config is in the deps.
        # It could also reference BuildConfig.COMMIT_ID.
        android_library(
          name = 'mylib',
          srcs = glob(['src/**/*.java']),
          deps = [
            ':build_config',
          ],
        )

        android_binary(
          name = 'debug',
          package_type = 'DEBUG',
          keystore =  '//keystores:debug',
          manifest = 'AndroidManifest.xml',
          target = 'Google Inc.:Google APIs:19',
          deps = [
            ':mylib',
          ],
        )

        # The contents of the file generated by this rule might be:
        #
        # String COMMIT_ID = "7bf804bdf71fdbfc99cce3b155b3643f022c6fa4"
        #
        # Note that the output of :build_config_release_values will be cached by Buck.
        # Assuming that generate_release_build_config.py depends on state that is not
        # expressed by its deps (which violates a fundamental invariant in Buck!), a
        # workaround is to ensure that the inputs to :build_config_release_values are
        # changed in some way before :release is built to ensure that the output from
        # :build_config_release_values is not pulled from cache. For example:
        #
        # $ buck build :release
        # $ uuidgen > dummy_state_file.txt
        # $ buck build :release
        #
        # This makes sure that generate_release_build_config.py is re-run before
        # :release is rebuilt. This is much cheaper than deleting your build cache
        # before rebuilding.
        genrule(
          name = 'build_config_release_values',
          srcs = [ 'generate_release_build_config.py', 'dummy_state_file.txt' ],
          bash = 'generate_release_build_config.py $OUT',
          out = 'build_config_release_values.txt',
        )

        android_binary(
          name = 'release',
          package_type = 'RELEASE',
          keystore =  '//keystores:release',
          manifest = 'AndroidManifest.xml',
          target = 'Google Inc.:Google APIs:19',
          build_config_values_file = ':build_config_release_values',
          deps = [
            ':mylib',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "package": attrs.string(
                default = "",
                doc = """
                Name of the Java package to use in the generated `BuildConfig.java` file.
                 Most developers set this to the application id declared in the manifest
                 via `<manifest package="APP_ID">`.
                 Example: `com.facebook.orca`.
            """,
            ),
            "values": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                List of strings that defines additional fields (and values) that should be declared in the
                 generated `BuildConfig.java` file. Like `DEBUG`, the values will be
                 non-constant-expressions that evaluate to the value specified in the file at compilation
                 time.
                 To override the values in an APK, specify build\\_config\\_values or build\\_config\\_values\\_file in `android_binary()`.
            """,
            ),
            "values_file": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                Optional path to a file that defines additional fields (and values) that should be declared in the
                 generated `BuildConfig.java` file. Like `DEBUG`, the values will be
                 non-constant-expressions that evaluate to the value specified in the file at compilation
                 time.
                 To override the values in an APK, specify build\\_config\\_values or build\\_config\\_values\\_file in `android_binary()`.
                 Binary values override matching fields; other fields from this file keep their values.

                 Note that values\\_file can be a generated file, as can build\\_config\\_values\\_file as
                 demonstrated in the example below.
            """,
            ),
            "_android_toolchain": toolchains_common.android(),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
            "_is_building_android_binary": is_building_android_binary_attr(),
            "_java_toolchain": toolchains_common.java_for_android(),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
