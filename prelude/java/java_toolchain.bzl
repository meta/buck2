# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

AbiGenerationMode = enum("class", "none", "source", "source_only")

DepFiles = enum("none", "per_class", "per_jar")

JavacProtocol = enum("classic", "javacd")

JavaPlatformInfo = provider(
    doc = "Java platform info",
    fields = {
        "name": provider_field(typing.Any, default = None),
    },
)

JavaToolchainInfo = provider(
    doc = "Java toolchain info",
    fields = {
        "abi_generation_mode": provider_field(typing.Any, default = None),
        "bootclasspath_7": provider_field(typing.Any, default = None),
        "bootclasspath_8": provider_field(typing.Any, default = None),
        "class_abi_generator": provider_field(typing.Any, default = None),
        "class_loader_bootstrapper": provider_field(typing.Any, default = None),
        "compile_and_package": provider_field(typing.Any, default = None),
        "compiler_class_to_source_map": provider_field(typing.Any, default = None),
        "cp_snapshot_generator": provider_field(typing.Any, default = None),
        "dep_files": provider_field(typing.Any, default = None),
        "fat_jar": provider_field(typing.Any, default = None),
        "fat_jar_main_class_lib": provider_field(typing.Any, default = None),
        "gen_class_to_source_map": provider_field(typing.Any, default = None),
        "gen_class_to_source_map_debuginfo": provider_field(typing.Any, default = None),  # optional
        "gen_class_to_source_map_include_sourceless_compiled_packages": provider_field(typing.Any, default = None),
        "global_code_config": provider_field(typing.Any, default = None),
        "is_bootstrap_toolchain": provider_field(typing.Any, default = None),
        "jar": provider_field(typing.Any, default = None),
        "jar_builder": provider_field(typing.Any, default = None),
        "java": provider_field(typing.Any, default = None),
        "java_base_jar": provider_field(typing.Any, default = None),
        "java_error_handler": provider_field(typing.Any, default = None),
        "java_for_tests": provider_field(typing.Any, default = None),
        # Major version of the JDK that `java` points at, or None when that is not known
        # (for example when a toolchain supplies its own `java` binary).
        "java_runtime_version": provider_field(typing.Any, default = None),
        "javac": provider_field(typing.Any, default = None),
        "javac_protocol": provider_field(typing.Any, default = None),
        "javacd": provider_field(typing.Any, default = None),
        # Java AOT cache for one-shot javacd JVMs, trained with `javacd_launcher_runtime`.
        "javacd_aot_cache": provider_field(typing.Any, default = None),
        "javacd_aot_mode": provider_field(str, default = "on"),
        "javacd_debug_port": provider_field(typing.Any, default = None),
        "javacd_debug_target": provider_field(typing.Any, default = None),
        "javacd_jvm_args": provider_field(typing.Any, default = None),
        "javacd_jvm_args_target": provider_field(typing.Any, default = None),
        # A `JvmLauncherRuntimeInfo` to start one-shot javacd JVMs from instead of `java -jar <bootstrapper>`.
        "javacd_launcher_runtime": provider_field(typing.Any, default = None),
        "javacd_main_class": provider_field(typing.Any, default = None),
        "javacd_remote_worker": provider_field(typing.Any, default = None),
        "javacd_worker": provider_field(typing.Any, default = None),
        "jdk_modules_jar": provider_field(typing.Any, default = None),
        "jlink": provider_field(typing.Any, default = None),
        "jmod": provider_field(typing.Any, default = None),
        "jrt_fs_jar": provider_field(typing.Any, default = None),
        "merge_class_to_source_maps": provider_field(typing.Any, default = None),
        "nullsafe": provider_field(typing.Any, default = None),
        "nullsafe_extra_args": provider_field(typing.Any, default = None),
        "nullsafe_signatures": provider_field(typing.Any, default = None),
        "postprocessor_runner": provider_field(typing.Any, default = None),
        "proguard_jar": provider_field(typing.Any, default = None),
        "proguard_max_heap_size": provider_field(typing.Any, default = None),
        "semanticdb_javac": provider_field(typing.Any, default = None),
        "semanticdb_sourceroot": provider_field(typing.Any, default = None),
        "source_level": provider_field(typing.Any, default = None),
        "src_root_elements": provider_field(typing.Any, default = None),
        "src_root_prefixes": provider_field(typing.Any, default = None),
        "target_level": provider_field(typing.Any, default = None),
        "target_stats_tools": provider_field(typing.Any, default = None),
        "track_class_usage": provider_field(bool, default = True),
        "uses_content_based_paths": provider_field(bool, default = True),
        "zip_scrubber": provider_field(typing.Any, default = None),
    },
)

def unsafe_memory_access_jvm_args(java_runtime_version: [int, None]) -> list[str]:
    """
    JVM args that keep the compiler daemon from warning about protobuf-java's use of
    sun.misc.Unsafe.

    protobuf-java, which carries the daemon's command protocol, calls
    sun.misc.Unsafe::arrayBaseOffset from a static initializer. JDK 24 and up print a
    terminal-deprecation warning for that call, and it lands in the stderr buck2 reports
    for a failed compilation. No protobuf-java release has dropped the call, so the
    warning has to be turned off at the JVM. The option does not exist before JDK 24,
    where passing it stops the JVM from starting at all.
    """
    if java_runtime_version != None and java_runtime_version >= 24:
        return ["--sun-misc-unsafe-memory-access=allow"]
    return []

JavaTestToolchainInfo = provider(
    doc = "Java test toolchain info",
    fields = {
        "junit5_test_runner_main_class_args": provider_field(typing.Any, default = None),
        "junit_test_runner_main_class_args": provider_field(typing.Any, default = None),
        "jvm_args": provider_field(typing.Any, default = None),
        "list_class_names": provider_field(typing.Any, default = None),
        "test_runner_library_jar": provider_field(typing.Any, default = None),
        "testng_test_runner_main_class_args": provider_field(typing.Any, default = None),
    },
)

# prebuilt_jar needs so little of the Java toolchain that it's worth
# giving it its own to reduce the occurrence of cycles as we add
# more Java- and Kotlin-built tools to the Java and Kotlin toolchains
PrebuiltJarToolchainInfo = provider(
    doc = "prebuilt_jar toolchain info",
    fields = {
        "class_abi_generator": provider_field(typing.Any, default = None),
        "cp_snapshot_generator": provider_field(typing.Any, default = None),
        "global_code_config": provider_field(typing.Any, default = None),
        "is_bootstrap_toolchain": provider_field(typing.Any, default = None),
        "java": provider_field(typing.Any, default = None),
    },
)
