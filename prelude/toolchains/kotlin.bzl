# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//java:java_toolchain.bzl", "DepFiles")
load("@prelude//kotlin:kotlin_toolchain.bzl", "KotlinToolchainInfo", "KotlincProtocol")
load("@prelude//toolchains/android/tools:kotlin_utils.bzl", "kotlin_version", "kotlin_version_at_least")

def _typechecker_enabled():
    # Buckconfig gate for the typecheck sub-target. Tools attach
    # only when enabled so the default graph carries zero new edges.
    return read_config("kotlin", "typechecker_enabled", "False").lower() in ("true", "1", "yes")

def _typechecker_cli():
    # Tool target is buckconfig-driven (default unset) so this open-source
    # prelude file never names a cell that may not exist: without [kotlin]
    # typechecker_cli the attr stays None and no sub-target is created.
    return read_config("kotlin", "typechecker_cli", None)

_KOSABI_PLUGINS = "prelude//toolchains/android/src/com/facebook/buck/jvm/kotlin/compilerplugins/kosabi"

_KOTLINCD_WORKER = "prelude//toolchains/android/src/com/facebook/buck/jvm/kotlin/cd/worker:kotlincd_grpc_worker_tool"

def _source_only_abi_enabled():
    # Whether Kosabi builds the source-only ABIs of targets with `abi_generation_mode = "source_only"`.
    # Without it those targets build class ABIs.
    return read_root_config("kotlin", "enable_source_only_abi", "True").lower() in ("true", "1", "yes")

def _persistent_workers_enabled():
    return read_root_config("build", "use_persistent_workers", "True").lower() in ("true", "1", "yes")

def kotlincd_toolchain(name, java_binary_for_kotlincd = None, visibility = None):
    source_only_abi = _source_only_abi_enabled()
    _kotlin_toolchain_rule(
        java_binary_for_kotlincd = java_binary_for_kotlincd,
        name = name,
        typechecker_cli = _typechecker_cli() if _typechecker_enabled() else None,
        typechecker_enabled = _typechecker_enabled(),
        typechecker_wrapper = "prelude//kotlin/tools/typechecker:typechecker" if _typechecker_enabled() else None,
        annotation_processing_jar = "prelude//toolchains/android/third-party:kotlin-annotation-processing-embeddable",
        class_loader_bootstrapper = "prelude//toolchains/android/src/com/facebook/buck/cli/bootstrapper:bootstrapper",
        compile_kotlin = "prelude//kotlin/tools/compile_kotlin:compile_kotlin",
        dep_files = "none",
        kapt_base64_encoder = "prelude//kotlin/tools/kapt_base64_encoder:kapt_base64_encoder",
        kotlin_stdlib = "prelude//toolchains/android/third-party:kotlin-stdlib",
        kotlin_version = kotlin_version(),
        kotlin_home_libraries = [
            "prelude//toolchains/android/third-party:kotlin-annotations",
            "prelude//toolchains/android/third-party:kotlin-build-tools-impl",
            "prelude//toolchains/android/third-party:kotlin-daemon-client",
            "prelude//toolchains/android/third-party:kotlin-compiler-embeddable",
            "prelude//toolchains/android/third-party:kotlin-reflect",
            "prelude//toolchains/android/third-party:kotlin-script-runtime",
            "prelude//toolchains/android/third-party:kotlin-stdlib",
            "prelude//toolchains/android/third-party:trove",
            "prelude//toolchains/android/third-party:kotlinx-coroutines-core-jvm",
        ]
        + (
            # kotlin-build-tools-impl 2.4+ loads kotlin-tooling-core at run time.
            ["prelude//toolchains/android/third-party:kotlin-tooling-core"] if kotlin_version_at_least(2, 4) else []
        ),
        kotlinc = "prelude//toolchains/android/third-party:kotlin-compiler-binary",
        kotlincd = "prelude//toolchains/android/src/com/facebook/buck/jvm/kotlin/cd/workertool:kotlincd_tool-main",
        kotlinc_protocol = "kotlincd",
        kotlincd_main_class = "com.facebook.buck.jvm.kotlin.cd.workertool.KotlinCDMain",
        kotlincd_worker = _KOTLINCD_WORKER if _persistent_workers_enabled() else None,
        kosabi_jvm_abi_gen_k2_plugin = _KOSABI_PLUGINS + "/k2-jvm-abi-gen:k2-jvm-abi-gen" if source_only_abi else None,
        kosabi_stubs_gen_k2_plugin = _KOSABI_PLUGINS + "/stubsgen:stubsgen_k2" if source_only_abi else None,
        visibility = visibility,
    )

def system_kotlin_bootstrap_toolchain(name, visibility = None):
    _kotlin_toolchain_rule(
        name = name,
        annotation_processing_jar = "prelude//toolchains/android/third-party:kotlin-annotation-processing-embeddable",
        compile_kotlin = "prelude//kotlin/tools/compile_kotlin:compile_kotlin",
        dep_files = "none",
        kapt_base64_encoder = "prelude//kotlin/tools/kapt_base64_encoder:kapt_base64_encoder",
        kotlin_stdlib = "prelude//toolchains/android/third-party:kotlin-stdlib",
        kotlin_version = kotlin_version(),
        kotlinc = "prelude//toolchains/android/third-party:kotlin-compiler-binary",
        kotlinc_protocol = "classic",
        visibility = visibility,
    )

def _kotlin_toolchain_rule_impl(ctx):
    return [
        DefaultInfo(),
        KotlinToolchainInfo(
            annotation_processing_jar = ctx.attrs.annotation_processing_jar,
            class_loader_bootstrapper = ctx.attrs.class_loader_bootstrapper,
            compile_kotlin = ctx.attrs.compile_kotlin,
            dep_files = DepFiles(ctx.attrs.dep_files),
            kapt_base64_encoder = ctx.attrs.kapt_base64_encoder,
            kotlinc = ctx.attrs.kotlinc,
            kotlincd = ctx.attrs.kotlincd,
            kotlin_stdlib = ctx.attrs.kotlin_stdlib,
            kotlin_version = ctx.attrs.kotlin_version,
            kotlin_home_libraries = ctx.attrs.kotlin_home_libraries,
            enable_incremental_compilation = ctx.attrs.enable_incremental_compilation or False,
            java_binary_for_kotlincd = ctx.attrs.java_binary_for_kotlincd,
            typechecker_cli = ctx.attrs.typechecker_cli,
            typechecker_enabled = ctx.attrs.typechecker_enabled or False,
            typechecker_wrapper = ctx.attrs.typechecker_wrapper,
            kotlinc_protocol = ctx.attrs.kotlinc_protocol,
            kosabi_stubs_gen_k2_plugin = ctx.attrs.kosabi_stubs_gen_k2_plugin,
            kosabi_applicability_plugin = ctx.attrs.kosabi_applicability_plugin,
            jvm_abi_gen_plugin = ctx.attrs.jvm_abi_gen_plugin,
            kotlincd_debug_port = ctx.attrs.kotlincd_debug_port,
            kotlincd_debug_target = ctx.attrs.kotlincd_debug_target,
            kotlincd_jvm_args = ctx.attrs.kotlincd_jvm_args,
            kotlincd_jvm_args_target = ctx.attrs.kotlincd_jvm_args_target,
            kotlincd_main_class = ctx.attrs.kotlincd_main_class,
            kotlincd_worker = ctx.attrs.kotlincd_worker,
            track_class_usage_plugin = ctx.attrs.track_class_usage_plugin,
            track_files_which_skipped_compilation = ctx.attrs.track_files_which_skipped_compilation or False,
            kotlin_error_handler = None,
            kosabi_jvm_abi_gen_k2_plugin = ctx.attrs.kosabi_jvm_abi_gen_k2_plugin,
            semanticdb_kotlinc = None,
            semanticdb_sourceroot = None,
        ),
    ]

_kotlin_toolchain_rule = rule(
    attrs = {
        "annotation_processing_jar": attrs.dep(),
        "class_loader_bootstrapper": attrs.option(attrs.source(), default = None),
        "compile_kotlin": attrs.dep(providers = [RunInfo]),
        "dep_files": attrs.enum(["none", "per_class", "per_jar"], default = "none"),
        "enable_incremental_compilation": attrs.option(attrs.bool(), default = None),
        "java_binary_for_kotlincd": attrs.option(attrs.dep(providers = [RunInfo]), default = None),
        "jvm_abi_gen_plugin": attrs.option(attrs.source(), default = None),
        "kapt_base64_encoder": attrs.dep(providers = [RunInfo]),
        "kosabi_applicability_plugin": attrs.option(attrs.source(), default = None),
        "kosabi_jvm_abi_gen_k2_plugin": attrs.option(attrs.source(), default = None),
        "kosabi_stubs_gen_k2_plugin": attrs.option(attrs.source(), default = None),
        "kotlin_home_libraries": attrs.list(attrs.source(), default = []),
        "kotlin_stdlib": attrs.dep(),
        "kotlin_version": attrs.string(),
        "kotlinc": attrs.dep(providers = [RunInfo]),
        "kotlinc_protocol": attrs.enum(KotlincProtocol.values(), default = "classic"),
        "kotlincd": attrs.option(attrs.dep(providers = [RunInfo]), default = None),
        "kotlincd_debug_port": attrs.option(attrs.int(), default = None),
        "kotlincd_debug_target": attrs.option(attrs.label(), default = None),
        "kotlincd_jvm_args": attrs.list(attrs.string(), default = []),
        "kotlincd_jvm_args_target": attrs.list(attrs.label(), default = []),
        "kotlincd_main_class": attrs.option(attrs.string(), default = None),
        "kotlincd_worker": attrs.option(attrs.dep(), default = None),
        "track_class_usage_plugin": attrs.option(attrs.source(), default = None),
        "track_files_which_skipped_compilation": attrs.option(attrs.bool(), default = None),
        "typechecker_cli": attrs.option(attrs.dep(providers = [RunInfo]), default = None),
        "typechecker_enabled": attrs.option(attrs.bool(), default = None),
        "typechecker_wrapper": attrs.option(attrs.dep(providers = [RunInfo]), default = None),
    },
    impl = _kotlin_toolchain_rule_impl,
    is_toolchain_rule = True,
)
