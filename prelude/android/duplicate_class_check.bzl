# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")

# Number of batches to split the class name files into.
# This reduces the number of hidden inputs from 40K+ files to NUM_BATCHES files.
_NUM_BATCHES = 32

def check_for_duplicate_classes_for_pre_dexed_libs(ctx: AnalysisContext, target_name_to_class_name_mapping: dict[str, Artifact]) -> Artifact:
    """
    Check for duplicate class names across pre-dexed libraries.

    To optimize performance, this function:
    1. Splits the class name files into batches
    2. Consolidates each batch into a single JSON file
    3. Runs the duplicate class checker on the consolidated files

    This reduces the number of hidden inputs from O(N) individual files to
    O(NUM_BATCHES) consolidated files, significantly improving action
    materialization time in remote execution.
    """
    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]

    # Convert to list for batching
    items = list(target_name_to_class_name_mapping.items())
    num_items = len(items)

    if num_items == 0:
        # No class names to check, just create empty validation output
        validation_output = ctx.actions.declare_output("validation_output.txt", has_content_based_path = False)
        ctx.actions.write(validation_output, "No duplicate class names found")
        return validation_output

    # Calculate batch size
    batch_size = max(1, (num_items + _NUM_BATCHES - 1) // _NUM_BATCHES)

    # Create consolidated files for each batch
    consolidated_files = []
    for batch_idx in range(0, num_items, batch_size):
        batch_end = min(batch_idx + batch_size, num_items)
        batch_items = items[batch_idx:batch_end]
        batch_mapping = {k: v for k, v in batch_items}

        batch_num = batch_idx // batch_size
        consolidated_file = _consolidate_class_names_batch(
            ctx,
            android_toolchain,
            batch_mapping,
            batch_num,
        )
        consolidated_files.append(consolidated_file)

    # Run duplicate class checker on consolidated files
    validation_output = ctx.actions.declare_output("validation_output.txt", has_content_based_path = False)
    consolidated_files_list = ctx.actions.declare_output("consolidated_files_list.txt", has_content_based_path = False)
    ctx.actions.write(
        consolidated_files_list,
        consolidated_files,
    )

    ctx.actions.run(
        cmd_args(
            android_toolchain.duplicate_class_checker[RunInfo],
            "--mode",
            "pre-dexed-libs",
            "--validation-output",
            validation_output.as_output(),
            "--consolidated-files-list",
            consolidated_files_list,
            hidden = consolidated_files,
        ),
        category = "check_duplicate_classes_pre_dexed_libs",
        allow_cache_upload = True,
    )
    return validation_output

def _consolidate_class_names_batch(
    ctx: AnalysisContext, android_toolchain: AndroidToolchainInfo, target_name_to_class_name_mapping: dict[str, Artifact], batch_num: int
) -> Artifact:
    """Consolidate a batch of class name files into a single JSON file."""
    input_mapping_file = ctx.actions.declare_output(
        "consolidate_batch_{}_input.json".format(batch_num),
        has_content_based_path = False,
    )
    ctx.actions.write_json(input_mapping_file, target_name_to_class_name_mapping)

    consolidated_output = ctx.actions.declare_output(
        "consolidated_class_names_batch_{}.json".format(batch_num),
        has_content_based_path = False,
    )

    ctx.actions.run(
        cmd_args(
            android_toolchain.consolidate_class_names[RunInfo],
            "--input-mapping-file",
            input_mapping_file,
            "--output-file",
            consolidated_output.as_output(),
            hidden = target_name_to_class_name_mapping.values(),
        ),
        category = "consolidate_class_names",
        identifier = "batch_{}".format(batch_num),
        allow_cache_upload = True,
    )
    return consolidated_output

def check_for_duplicate_classes_for_non_pre_dexed_jars(ctx: AnalysisContext, jar_to_owning_target_mapping: dict[Artifact, TargetLabel]) -> Artifact:
    validation_output = ctx.actions.declare_output("validation_output.txt", has_content_based_path = False)
    jar_to_owning_target_map_file = ctx.actions.declare_output("jar_to_owning_target_map_file.txt", has_content_based_path = False)
    ctx.actions.write_json(
        jar_to_owning_target_map_file,
        jar_to_owning_target_mapping,
    )

    ctx.actions.run(
        cmd_args(
            ctx.attrs._android_toolchain[AndroidToolchainInfo].duplicate_class_checker[RunInfo],
            "--mode",
            "non-pre-dexed-jars",
            "--jar-to-owning-target-map-file",
            jar_to_owning_target_map_file,
            "--validation-output",
            validation_output.as_output(),
            hidden = jar_to_owning_target_mapping.keys(),
        ),
        category = "check_duplicate_classes_non_pre_dexed_jars",
    )
    return validation_output
