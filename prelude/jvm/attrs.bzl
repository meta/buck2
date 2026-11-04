# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//android:min_sdk_version.bzl", "get_min_sdk_version_constraint_value_name", "get_min_sdk_version_range")
load("@prelude//decls:common.bzl", "AbiGenerationMode")

def _test_env():
    return {
        "env": attrs.dict(
            key = attrs.string(),
            value = attrs.arg(),
            sorted = False,
            default = {},
            doc = """
    A map of environment names and values to set when running the test.
""",
        ),
    }

def _resources_arg():
    return {
        "resources": attrs.list(
            attrs.source(),
            default = [],
            doc = """
    Static files to include with the compiled `.class` files.
     These files can be loaded
     via [Class.getResource()](http://docs.oracle.com/javase/7/docs/api/java/lang/Class.html#getResource(java.lang.String)).

    **Note:** If `resources_root` isn't set,
     Buck uses the ``.buckconfig``
     property in `.buckconfig` to
     determine where resources should be placed within the generated JAR
     file.
""",
        ),
        "resources_root": attrs.option(
            attrs.source(),
            default = None,
            doc = """
    The path that resources are resolved against. For example, if `resources_root` is `"res"` and
     `resources` contains the file `"res/com/example/foo.txt"`, that file will end up as `"com/example/foo.txt"` in the output JAR. This parameter
     overrides the ``.buckconfig`` property in `.buckconfig`.
""",
        ),
    }

def _remove_classes_arg():
    return {
        "remove_classes": attrs.list(
            attrs.regex(),
            default = [],
            doc = """
    Specifies a list of `Patterns` that are used to exclude
     `classes` from the `JAR`. The pattern matching is
     based on the name of the class. This can be used to exclude a member
     class or delete a local view of a class that will be replaced during
     a later stage of the build.
""",
        ),
    }

def _provided_deps():
    return {
        "provided_deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    These represent dependencies that are known to be provided at run
     time, but are required in order for the code to compile. Examples of
     `provided_deps` include the JEE servlet APIs. When this
     rule is included in a , the
     `provided_deps` will not be packaged into the output.
""",
        ),
    }

def _exported_deps():
    return {
        "exported_deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    Other  rules that depend on this rule will also
     include its `exported_deps` in their classpaths. This is useful
     when the public API of a rule has return types or checked exceptions that are
     defined in another rule, which would otherwise require callers to add an
     extra dependency. It's also useful for exposing e.g. a collection of
     `prebuilt_jar` rules as a single target for callers to depend
     on. Targets in `exported_deps` are implicitly included in the
     `deps` of this rule, so they don't need to be repeated there.
""",
        ),
    }

def _multi_release_jar():
    return {
        "min_release_version": attrs.option(
            attrs.string(),
            default = None,
            doc = """
                Minimal java version that the library should be compatible with.
                This ensures that both the language features and the bytecode generated are
                compatible with the specified Java version, and only APIs available in that version are accessible.
            """,
        ),
        "multi_release_srcs": attrs.dict(
            key = attrs.string(),
            value = attrs.list(attrs.source(), default = []),
            default = {},
            doc = """
                The set of `.java` files to compile for the mapped java version.
                Each version will be packed under META-INF/versions/java_version.
            """,
        ),
    }

def _exported_provided_deps():
    return {
        "exported_provided_deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    This is a combination of `provided_deps` and `exported_deps`. Rules listed
     in this parameter will be added to classpath of rules that depend on this rule, but they will not
     be included in a binary if binary depends on a such target.
""",
        ),
    }

def _source_only_abi_deps():
    return {
        "source_only_abi_deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    These are dependencies that must be present during
     `source-only ABI generation`.
     Typically such dependencies are added when some property of the code in this rule prevents source-only ABI
     generation from being correct without these dependencies being present.

     Having `source_only_abi_deps` prevents Buck from
     completely flattening the build graph, thus reducing the performance win from source-only
     ABI generation. They should be avoided when possible. Often only a small code change is needed to avoid them.
     For more information on such code changes, read about
     `source-only ABI generation`.
""",
        ),
    }

def _abi_generation_mode():
    return {
        "abi_generation_mode": attrs.option(
            attrs.enum(AbiGenerationMode),
            default = None,
            doc = """
    Overrides `.buckconfig`
    for this rule.
""",
        ),
    }

def _required_for_source_only_abi():
    return {
        "required_for_source_only_abi": attrs.bool(
            default = False,
            doc = """
    Indicates that this rule must be present on the classpath during
     `source-only ABI generation`
     of any rule that depends on it. Typically this is done when a rule contains annotations,
     enums, constants, or interfaces.

     Having rules present on the classpath during source-only ABI generation prevents Buck from
     completely flattening the build graph, thus reducing the performance win from source-only
     ABI generation. These rules should be kept small (ideally just containing annotations,
     constants, enums, and interfaces) and with minimal dependencies of their own.
""",
        ),
    }

def _incremental():
    return {
        "incremental": attrs.bool(
            default = False,
            doc = """
                Enables Kotlin incremental compilation.
            """,
        ),
    }

def _plugins():
    return {
        "non_exec_dep_plugins_deprecated": attrs.list(
            attrs.one_of(
                attrs.dep(),
                attrs.tuple(attrs.dep(), attrs.list(attrs.string())),
            ),
            default = [],
            doc = """
                Plugins that do not use the execution platform. This exists for historical reasons,
                and should not be used. Use `plugins` instead - plugins should be configured for
                the execution platform since that is where they are used.
                """,
        ),
        "plugins": attrs.list(
            attrs.one_of(
                attrs.exec_dep(),
                attrs.tuple(attrs.exec_dep(), attrs.list(attrs.string())),
            ),
            default = [],
            doc = """
                List of plugins that should be run during compilation of the target. A list of
                strings may additionally be provided in order to pass additional arguments to
                the plugin.
                """,
        ),
    }

def _kotlin_compiler_plugins():
    return {
        "kotlin_compiler_plugins": attrs.list(
            attrs.tuple(attrs.exec_dep(), attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False)),
            default = [],
            doc = """
                Use this to specify [Kotlin compiler plugins](https://kotlinlang.org/docs/reference/compiler-plugins.html) to use when compiling this library.
                 This takes a map, with each entry specify one plugin. Entry's key is plugin source path,
                 and value is a map of plugin option key value pair. Unlike `extra_kotlinc_arguments`,
                 these can be *source paths*, not just strings.

                 A special option value is
                 `__codegen_dir__`, in which case Buck will provide a default codegen folder's path as
                 option value instead.
                 E.g.

                ```
fbcode/buck2/prelude/decls/jvm_common.bzl
                kotlin_compiler_plugins = {
                    "somePluginSourcePath": {
                        "plugin:somePluginId:somePluginOptionKey": "somePluginOptionValue",
                        "plugin:somePluginId:someDirectoryRelatedOptionKey": "__codegen_dir__",
                    },
                },
                ```
                Each plugin source path will be prefixed with `-Xplugin=` and passed as extra
                 arguments to the compiler. Plugin options will be appended after its plugin with `-P`.

                 A specific example is, if you want to use [kotlinx.serialization](https://github.com/Kotlin/kotlinx.serialization)
                 with `kotlin_library()`, you need to specify `kotlinx-serialization-compiler-plugin.jar` under `kotlin_compiler_plugins` and `kotlinx-serialization-runtime.jar` (which you may have to fetch from Maven) in your `deps`:

                ```
                kotlin_library(
                    name = "example",
                    srcs = glob(["*.kt"]),
                    deps = [
                        ":kotlinx-serialization-runtime",
                    ],
                    kotlin_compiler_plugins = {
                        # Likely copied from your $KOTLIN_HOME directory.
                        "kotlinx-serialization-compiler-plugin.jar": {},
                    },
                )

                prebuilt_jar(
                    name = "kotlinx-serialization-runtime",
                    binary_jar = ":kotlinx-serialization-runtime-0.10.0",
                )

                # Note you probably want to set
                # maven_repo=http://jcenter.bintray.com/ in your .buckconfig until
                # https://github.com/Kotlin/kotlinx.serialization/issues/64
                # is closed.
                remote_file(
                    name = "kotlinx-serialization-runtime-0.10.0",
                    out = "kotlinx-serialization-runtime-0.10.0.jar",
                    url = "mvn:org.jetbrains.kotlinx:kotlinx-serialization-runtime:jar:0.10.0",
                    sha1 = "23d777a5282c1957c7ce35946374fff0adab114c"
                )
                ```
            """,
        ),
    }

def _annotation_processors():
    return {
        "annotation_processor_deps": attrs.list(attrs.exec_dep(), default = []),
        "annotation_processor_params": attrs.list(attrs.string(), default = []),
        "annotation_processors": attrs.list(attrs.string(), default = []),
        "source_only_abi_annotation_processor_param_overrides": attrs.dict(
            key = attrs.string(),
            value = attrs.option(attrs.string()),
            sorted = False,
            default = {},
            doc = "Parameter overrides for source-only ABI; `None` removes a parameter.",
        ),
    }

def _content_based_path_attr():
    return attrs.bool(
        default = select({
            "DEFAULT": False,
            # @oss-disable[end= ]: "config//build_mode/constraints:_whatsapp[whatsapp]": True,
            # @oss-disable[end= ]: "config//build_mode:arvr_mode[enabled]": True,
            # @oss-disable[end= ]: "config//os/constraints:android": True,
            # @oss-disable[end= ]: "config//runtime/constraints:android-host-test": True,
            # @oss-disable[end= ]: "config//runtime/constraints:android-unit-test": True,
        })
    )

def _classic_java_content_based_paths():
    return {
        "uses_content_based_paths_for_classic_java": _content_based_path_attr(),
    }

def _javac():
    return {
        "javac": attrs.option(
            attrs.one_of(attrs.exec_dep(), attrs.source()),
            default = None,
            doc = """
            Specifies the Java compiler program to use for this rule.
                The value is a source path or an execution dep (e.g., //foo/bar:bar).
                Overrides the value in "javac" in the "tools" section
                of `.buckconfig`.
            """,
        ),
    }

def _friend_paths_attr():
    return attrs.list(
        attrs.one_of(attrs.dep(), attrs.string()),
        default = [],
        doc = """
    Modules whose `internal` declarations this rule may access, passed to the Kotlin compiler as
     friend-paths.

    An entry is either a target (`//foo/bar:baz`) or a build target pattern covering more than one
     target (`//foo/bar/...` recursively, or `//foo/bar:` for a single package).
""",
    )

jvm_common = struct(
    test_env = _test_env,
    friend_paths_attr = _friend_paths_attr,
    resources_arg = _resources_arg,
    remove_classes_arg = _remove_classes_arg,
    provided_deps = _provided_deps,
    exported_deps = _exported_deps,
    exported_provided_deps = _exported_provided_deps,
    source_only_abi_deps = _source_only_abi_deps,
    abi_generation_mode = _abi_generation_mode,
    required_for_source_only_abi = _required_for_source_only_abi,
    incremental = _incremental,
    plugins = _plugins,
    kotlin_compiler_plugins = _kotlin_compiler_plugins,
    annotation_processors = _annotation_processors,
    javac = _javac,
    multi_release_jar = _multi_release_jar,
    classic_java_content_based_paths = _classic_java_content_based_paths,
    content_based_path_attr = _content_based_path_attr,
)

def min_sdk_version():
    min_sdk_version_dict = {"DEFAULT": None}
    for min_sdk in get_min_sdk_version_range():
        constraint = "prelude//android/constraints:{}".format(get_min_sdk_version_constraint_value_name(min_sdk))
        min_sdk_version_dict[constraint] = min_sdk

    return select(min_sdk_version_dict)
