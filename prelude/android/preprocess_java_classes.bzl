# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//android:android_providers.bzl", "AndroidPreprocessedJavaClassesInfo")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:util.bzl", "EnhancementContext")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//java/utils:java_more_utils.bzl", "get_path_separator_for_exec_os")
load("@prelude//utils:expect.bzl", "expect")

def get_preprocessed_java_classes(
    enhance_ctx: EnhancementContext,
    input_jars: dict[Artifact, TargetLabel],
) -> (dict[Artifact, TargetLabel], AndroidPreprocessedJavaClassesInfo | None):
    if not input_jars:
        return {}, None

    ctx = enhance_ctx.ctx

    input_srcs = {}
    output_jars_to_owners = {}
    output_dir = ctx.actions.declare_output("preprocessed_java_classes/output_dir", has_content_based_path = False)
    input_jars_to_owners = {}

    for i, (input_jar, target_label) in enumerate(input_jars.items()):
        expect(input_jar.extension == ".jar", "Expected {} to have extension .jar!".format(input_jar))
        jar_name = "{}_{}".format(i, input_jar.basename)
        input_srcs[jar_name] = input_jar
        input_jars_to_owners[jar_name] = target_label
        output_jar = output_dir.project(jar_name)
        output_jars_to_owners[output_jar] = target_label

    input_dir = ctx.actions.symlinked_dir("preprocessed_java_classes/input_dir", input_srcs, has_content_based_path = False)
    input_jars_map = ctx.actions.write_json("preprocessed_java_classes/input_jars_map.json", input_jars_to_owners, has_content_based_path = False)
    materialized_artifacts_dir = ctx.actions.declare_output("preprocessed_java_classes/materialized_artifacts", has_content_based_path = False)

    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]
    env = {
        "ANDROID_BOOTCLASSPATH": cmd_args(
            android_toolchain.android_bootclasspath + android_toolchain.android_optional_jars,
            delimiter = get_path_separator_for_exec_os(ctx),
        ),
        "IN_JARS_DIR": cmd_args(input_dir),
        "IN_JARS_MAP": cmd_args(input_jars_map),
        "MATERIALIZED_ARTIFACTS_DIR": materialized_artifacts_dir.as_output(),
        "OUT_JARS_DIR": output_dir.as_output(),
        "PREPROCESS": ctx.attrs.preprocess_java_classes_bash,
        "ZIP_SCRUBBER": ctx.attrs._java_toolchain[JavaToolchainInfo].zip_scrubber,
    }

    output_jars = output_jars_to_owners.keys()
    output_jars_file = ctx.actions.write("preprocessed_java_classes/output_jars.txt", output_jars, has_content_based_path = False)

    preprocess_cmd = [
        "/usr/bin/env",
        "bash",
        "-c",
        # Note: ZIP_SCRUBBER might expand to multiple words, so no quoting there.
        'mkdir -p "$OUT_JARS_DIR" && mkdir -p "$MATERIALIZED_ARTIFACTS_DIR" && eval "$PREPROCESS" && $ZIP_SCRUBBER --paths-to-scrub "$@"',
        "--",
        output_jars_file,
    ]

    preprocess_cmd = cmd_args(
        preprocess_cmd,
        hidden = [output_jar.as_output() for output_jar in output_jars]
        + [dep[DefaultInfo].default_outputs + dep[DefaultInfo].other_outputs for dep in ctx.attrs.preprocess_java_classes_deps],
    )

    ctx.actions.run(preprocess_cmd, env = env, category = "preprocess_java_classes")

    enhance_ctx.debug_output("preprocess_java_classes_input_dir", input_dir)
    enhance_ctx.debug_output("preprocess_java_classes_input_jars_map", input_jars_map)
    enhance_ctx.debug_output("preprocess_java_classes_materialized_artifacts_dir", materialized_artifacts_dir)

    return output_jars_to_owners, AndroidPreprocessedJavaClassesInfo(
        input_dir = input_dir,
        materialized_artifacts_dir = materialized_artifacts_dir,
    )
