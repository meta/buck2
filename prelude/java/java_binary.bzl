# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//android:configuration.bzl", "is_building_android_binary_attr")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//java:proguard.bzl", "get_proguard_output")
load("@prelude//java/utils:java_utils.bzl", "get_class_to_source_map_info", "get_classpath_subtargets")
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibrary",  # @unused used as type
    "SharedLibraryInfo",
    "merge_shared_libraries",
    "traverse_shared_library_info",
)
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")
load("@prelude//utils:expect.bzl", "expect")
load(
    ":java_providers.bzl",
    "create_template_info",
    "get_compiling_deps_tset",
    "get_java_packaging_info",
)

def _generate_script(generate_wrapper: bool, native_libs: list[SharedLibrary]) -> bool:
    # if `generate_wrapper` is set and no native libs then it should be a wrapper script as result,
    # otherwise fat jar will be generated (inner jar or script will be included inside a final fat jar)
    return generate_wrapper and len(native_libs) == 0

def _create_fat_jar(
    ctx: AnalysisContext,
    jars: cmd_args,
    native_libs: list[SharedLibrary] = [],
    name_prefix: str = "",
    concat_jars: bool = False,
    do_not_create_inner_jar: bool = True,
    generate_wrapper: bool = False,
    main_class: [str, None] = None,
    append_jar: [Artifact, None] = None,
) -> list[Artifact]:
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    extension = "sh" if _generate_script(generate_wrapper, native_libs) else "jar"
    output = ctx.actions.declare_output("{}{}.{}".format(name_prefix, ctx.label.name, extension), has_content_based_path = False)

    args = [
        java_toolchain.fat_jar[RunInfo],
        "--jar_builder_tool",
        cmd_args(java_toolchain.jar_builder, delimiter = " "),
        "--zip_scrubber_tool",
        cmd_args(java_toolchain.zip_scrubber, delimiter = " "),
        "--output",
        output.as_output(),
        "--jars_file",
        ctx.actions.write("{}jars_file".format(name_prefix), jars, has_content_based_path = False),
    ]

    if concat_jars:
        args += ["--concat_jars"]
    if append_jar:
        args += ["--append_jar", append_jar]

    if native_libs:
        expect(
            java_toolchain.is_bootstrap_toolchain == False,
            "Bootstrap java toolchain could not be used for java_binary() with native code.",
        )
        expect(
            not (generate_wrapper and do_not_create_inner_jar),
            "`generate_wrapper` cannot be combined with `do_not_create_inner_jar` for a java_binary() with native code: a wrapper script has nowhere to put the native libraries.",
        )
        args += [
            "--native_libs_file",
            ctx.actions.write(
                "{}native_libs".format(name_prefix),
                [cmd_args([native_lib.soname.ensure_str(), native_lib.lib.output], delimiter = " ") for native_lib in native_libs],
                has_content_based_path = False,
            ),
        ]
        if do_not_create_inner_jar:
            args += [
                "--do_not_create_inner_jar",
            ]
        else:
            args += [
                "--fat_jar_lib",
                java_toolchain.fat_jar_main_class_lib,
                # fat jar's main class
                "--fat_jar_main_class",
                "com.facebook.buck.jvm.java.fatjar.FatJarMain",
                # native libraries directory name. Main class expects to find libraries packed inside this directory.
                "--fat_jar_native_libs_directory_name",
                "nativelibs",
            ]

    if main_class:
        args += ["--main_class", main_class]

    if ctx.attrs.manifest_file:
        args += ["--manifest", ctx.attrs.manifest_file]

    build_manifest = ctx.attrs.build_manifest
    if build_manifest:
        args += ["--build_manifest", build_manifest]

    blocklist = ctx.attrs.blocklist
    if blocklist:
        args += ["--blocklist", ctx.actions.write("{}blocklist_args".format(name_prefix), blocklist, has_content_based_path = False)]

    if ctx.attrs.meta_inf_directory:
        args += ["--meta_inf_directory", ctx.attrs.meta_inf_directory]

    outputs = [output]
    if generate_wrapper:
        classpath_args_output = ctx.actions.declare_output("classpath_args", has_content_based_path = False)
        args += [
            "--generate_wrapper",
            "--classpath_args_output",
            classpath_args_output.as_output(),
            "--java_tool",
            java_toolchain.java[RunInfo],
            "--script_marker_file_name",
            "wrapper_script",
        ]
        outputs.append(classpath_args_output)

    fat_jar_cmd = cmd_args(
        args,
        hidden = [jars] + [native_lib.lib.output for native_lib in native_libs],
    )

    ctx.actions.run(
        fat_jar_cmd,
        local_only = False,
        category = "{}fat_jar".format(name_prefix),
        allow_cache_upload = True,
    )

    if generate_wrapper == False:
        expect(
            len(outputs) == 1,
            "expected exactly one output when creating a fat jar",
        )

    # If `generate_wrapper` is not set then the result will contain only 1 item that represent fat jar artifact.
    # Else if `generate_wrapper` is set then the first item in the result list will be script or far jar, and the second one is for @classpath_args file
    return outputs

def _get_run_cmd(attrs: struct, script_mode: bool, main_artifact: Artifact, java_toolchain: JavaToolchainInfo) -> cmd_args:
    if script_mode:
        return cmd_args(["/usr/bin/env", "bash", main_artifact])
    else:
        return cmd_args([java_toolchain.java[RunInfo]] + attrs.java_args_for_run_info + ["-jar", main_artifact])

def _get_java_tool_artifacts(java_toolchain: JavaToolchainInfo) -> list:
    default_info = java_toolchain.java[DefaultInfo]
    return default_info.default_outputs + default_info.other_outputs

def java_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    """
     java_binary() rule implementation

    Args:
        ctx: rule analysis context
    Returns:
        list of created providers (DefaultInfo and RunInfo)
    """

    if ctx.attrs._build_only_native_code:
        return [
            DefaultInfo(default_output = ctx.actions.write("{}/unused.jar".format(ctx.label.name), [], has_content_based_path = False)),
            RunInfo(),
        ]

    packaging_info = get_java_packaging_info(ctx, ctx.attrs.deps, None)

    first_order_deps = get_compiling_deps_tset(ctx.actions, ctx.attrs.deps)
    first_order_libs = [dep.full_library for dep in (list(first_order_deps.traverse()) if first_order_deps else [])]

    shared_library_info = merge_shared_libraries(
        ctx.actions,
        deps = filter(None, [x.get(SharedLibraryInfo) for x in ctx.attrs.deps]),
    )
    native_deps = traverse_shared_library_info(shared_library_info, transformation_provider = None)

    base_dep = ctx.attrs.base_dep
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    concat_deps = ctx.attrs.concat_deps
    need_to_generate_wrapper = ctx.attrs.generate_wrapper == True
    do_not_create_inner_jar = ctx.attrs.do_not_create_inner_jar == True
    packaging_deps = list(packaging_info.packaging_deps.traverse())
    incremental_target_prefix = ctx.attrs.incremental_target_prefix
    main_class = ctx.attrs.main_class
    packaging_dep_infos = {dep.jar: dep.label.raw_target() for dep in packaging_deps if dep and dep.jar}

    other_outputs = []

    if ctx.attrs.proguard_config:
        java_base = [java_toolchain.java_base_jar] if java_toolchain.java_base_jar else []
        library_jars = ctx.attrs.proguard_library_jars + java_base
        proguard_output = get_proguard_output(
            ctx = ctx,
            input_jars = packaging_dep_infos,
            java_packaging_deps = packaging_deps,
            additional_proguard_configs = [],
            additional_jars = library_jars,
            sdk_proguard_config_mode = None,
            sdk_proguard_config = None,
            sdk_optimized_proguard_config = None,
            proguard_jar = java_toolchain.proguard_jar,
            skip_proguard = False,
        )
        packaging_dep_infos = proguard_output.jars_to_owners
    packaging_jar_args = cmd_args(packaging_dep_infos.keys())

    if incremental_target_prefix:
        base_jar = None
        incremental_jars = []
        dependency_jars = []

        # separate jars in groups
        for dep_jar, owner in packaging_dep_infos.items():
            # lookup for the base jar that can be used to append all other dependencies
            if base_dep and owner == base_dep.label.raw_target():
                expect(
                    base_jar == None,
                    "JAR can only have one base JAR file.",
                )
                base_jar = dep_jar
            elif str(owner).startswith(incremental_target_prefix):
                # if it's not a base jar, it can be an incremental jar or dependency only
                incremental_jars.append(dep_jar)
            else:
                dependency_jars.append(dep_jar)

        # collect incremental targets
        expect(
            len(incremental_jars) > 0,
            "No incremental dependencies found that starts with {}.".format(incremental_target_prefix),
        )

        # generate intermediary jar only with dependencies
        deps_outputs = _create_fat_jar(
            ctx,
            cmd_args(dependency_jars),
            concat_jars = concat_deps,
            name_prefix = "deps_",
            append_jar = base_jar,
        )
        other_outputs = [deps_outputs[0]]

        # generate final jar appending modules to the dependencies jar
        outputs = _create_fat_jar(
            ctx,
            cmd_args(incremental_jars),
            native_libs = native_deps,
            do_not_create_inner_jar = do_not_create_inner_jar,
            generate_wrapper = need_to_generate_wrapper,
            main_class = main_class,
            append_jar = deps_outputs[0],
        )
    else:
        outputs = _create_fat_jar(
            ctx,
            packaging_jar_args,
            concat_jars = concat_deps,
            native_libs = native_deps,
            do_not_create_inner_jar = do_not_create_inner_jar,
            generate_wrapper = need_to_generate_wrapper,
            main_class = main_class,
        )

    run_cmd = _get_run_cmd(
        attrs = ctx.attrs,
        script_mode = _generate_script(need_to_generate_wrapper, native_deps),
        main_artifact = outputs[0],
        java_toolchain = java_toolchain,
    )

    if need_to_generate_wrapper:
        classpath_file = outputs[1]
        run_cmd.add(
            cmd_args(
                hidden = [
                    java_toolchain.java[RunInfo],
                    classpath_file,
                    packaging_jar_args,
                ]
            )
        )
        other_outputs = [classpath_file] + [packaging_jar_args] + _get_java_tool_artifacts(java_toolchain)

    sub_targets = get_classpath_subtargets(ctx.actions, packaging_info)

    class_to_src_map, _, _ = get_class_to_source_map_info(
        ctx,
        outputs = None,
        deps = ctx.attrs.deps,
    )

    return [
        DefaultInfo(default_output = outputs[0], other_outputs = other_outputs, sub_targets = sub_targets),
        RunInfo(args = run_cmd),
        create_template_info(ctx, packaging_info, first_order_libs),
        class_to_src_map,
    ]

java_binary = prelude_rule(
    name = "java_binary",
    impl = java_binary_impl,
    docs = """
        A `java_binary()` rule is used to create a JAR file of the
        compiled .class files and resources of the `java_library()`
         rules on which it depends.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "base_dep": attrs.option(
                attrs.dep(),
                default = None,
                doc = """
                Rule (normally of type `java_library`) that should be
                 compiled and used as a base JAR to receive all dependencies through an append operation.
            """,
            ),
            "build_manifest": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                MANIFEST containing stamped build attributes, that should be merged into the main jar manifest
            """,
            ),
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Rules (normally of type `java_library`) that should be
                 compiled and whose `.class` files and resources should be
                 included in the generated JAR file.
            """,
            ),
            "main_class": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                If provided, this will be the value specified as the
                 `Main-Class` attribute of the `META-INF/MANIFEST.MF`
                 file in the generated JAR file. Also, when this rule is used as
                 an executable in a `genrule()`,
                 `main_class` will indicate the class whose `main()` method will be invoked to process the command-line arguments. This
                 is consistent with the expected usage of `java -jar
                 *<name.jar>* *<args>*`.
            """,
            ),
            "manifest_file": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                If provided, this manifest will be used when generating the JAR
                 file. If combined with `main_class`, the specified manifest
                 file will be used but the `main_class` will override the main
                 class in the manifest.
            """,
            ),
            "java_args_for_run_info": attrs.list(attrs.string(), default = []),
            "meta_inf_directory": attrs.option(
                attrs.source(allow_directory = True),
                default = None,
                doc = """
                Note: This has beta support currently.
                 If provided, the contents in this directory will end up in the
                 `META-INF` directory inside the generated JAR file.
            """,
            ),
            "blocklist": attrs.list(
                attrs.regex(),
                default = [],
                doc = """
                A list of patterns that identify files to exclude from the final generated JAR
                 file. Example:

                ```
                java_binary(
                  name = 'example',
                  blocklist = [
                    # Excludes com.example.A and com.example.Alligator,
                    # as well as their inner classes and any non-class files that happen to match
                    # the pattern
                    'com.example.A',

                    # Excludes all files from org/slf4j/**/*.
                    'org.slf4j',
                  ],
                  deps = [
                    ':example1',
                    ':third-party-stuff',
                  ],
                )
                ```
            """,
            ),
            "concat_deps": attrs.bool(default = False, doc = "Use zip concatenation instead of repacking all dependency jars, which is faster"),
            "default_cxx_platform": attrs.option(attrs.string(), default = None),
            "generate_wrapper": attrs.bool(
                default = False,
                doc = """
                Produce a shell script that runs the classes from a classpath file instead of
                packing them into a fat jar. With native libraries the output is still a fat
                jar, since a script has nowhere to put them.
            """,
            ),
            "do_not_create_inner_jar": attrs.bool(
                default = False,
                doc = """
                With native libraries, place them in a `nativelibs/` directory of the fat jar
                and load them from resources, instead of nesting the classes in an `inner.jar`
                behind a launcher main class. Entries then pass through `blocklist`. Cannot be
                combined with `generate_wrapper`.
            """,
            ),
            "incremental_target_prefix": attrs.option(attrs.string(), default = None),
            "java_version": attrs.option(attrs.string(), default = None, doc = "Expected java version used at compile time"),
            "java_runtime": attrs.option(attrs.string(), default = None, doc = "Expected java version used at runtime"),
            "proguard_config": attrs.option(attrs.source(), default = None),
            "proguard_jvm_args": attrs.list(attrs.string(), default = []),
            "proguard_library_jars": attrs.list(attrs.source(), default = []),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_is_building_android_binary": is_building_android_binary_attr(),
            "_java_toolchain": toolchains_common.java(),
        }
        | constraint_overrides.attributes
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
    cfg = constraint_overrides.transition,
)
