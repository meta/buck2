# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

KotlincProtocol = enum("classic", "kotlincd")

KotlinToolchainInfo = provider(
    doc = "Kotlin toolchain info",
    fields = {
        "annotation_processing_jar": provider_field(typing.Any, default = None),
        "class_loader_bootstrapper": provider_field(typing.Any, default = None),
        "compile_kotlin": provider_field(typing.Any, default = None),
        "dep_files": provider_field(typing.Any, default = None),
        "enable_incremental_compilation": provider_field(typing.Any, default = None),
        "java_binary_for_kotlincd": provider_field(typing.Any, default = None),
        "jvm_abi_gen_plugin": provider_field(typing.Any, default = None),
        "kapt_base64_encoder": provider_field(typing.Any, default = None),
        "kosabi_abi_validation_mode": provider_field(str, default = "off"),
        "kosabi_applicability_plugin": provider_field(typing.Any, default = None),
        "kosabi_jvm_abi_gen_k2_plugin": provider_field(typing.Any, default = None),
        "kosabi_shallow_class_usage": provider_field(bool, default = False),
        "kosabi_stubgen_engine": provider_field(str, default = "kotlinc"),
        "kosabi_stubs_gen_k2_plugin": provider_field(typing.Any, default = None),
        "kotlin_error_handler": provider_field(typing.Any, default = None),
        "kotlin_home_libraries": provider_field(typing.Any, default = None),
        "kotlin_stdlib": provider_field(typing.Any, default = None),
        "kotlin_version": provider_field(str),
        "kotlinc": provider_field(typing.Any, default = None),
        "kotlinc_protocol": provider_field(typing.Any, default = None),
        "kotlincd": provider_field(typing.Any, default = None),
        "kotlincd_debug_port": provider_field(typing.Any, default = None),
        "kotlincd_debug_target": provider_field(typing.Any, default = None),
        "kotlincd_jvm_args": provider_field(typing.Any, default = None),
        "kotlincd_jvm_args_target": provider_field(typing.Any, default = None),
        "kotlincd_main_class": provider_field(typing.Any, default = None),
        "kotlincd_remote_worker": provider_field(typing.Any, default = None),
        # JVM arguments for kotlincd runs outside persistent workers (remote execution) of compiles with at most
        # `kotlincd_small_action_max_srcs` sources: such a JVM lives a few seconds and spends most of its CPU in the JIT.
        "kotlincd_small_action_jvm_args": provider_field(list[str], default = []),
        "kotlincd_small_action_max_srcs": provider_field(int, default = 0),
        "kotlincd_worker": provider_field(typing.Any, default = None),
        "rokoko_stubgen_so": provider_field(typing.Any, default = None),
        "semanticdb_kotlinc": provider_field(typing.Any, default = None),
        "semanticdb_sourceroot": provider_field(typing.Any, default = None),
        "track_class_usage_plugin": provider_field(typing.Any, default = None),
        "track_files_which_skipped_compilation": provider_field(bool, default = False),
        "typechecker_cli": provider_field(typing.Any, default = None),
        "typechecker_enabled": provider_field(bool, default = False),
        "typechecker_wrapper": provider_field(typing.Any, default = None),
    },
)
