# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbcode//buck2/tests:buck_e2e.bzl", "buck2_e2e_test")
load("@fbcode_macros//build_defs:export_files.bzl", "export_file")

def bxl_test(src, name = None, labels = None, buck_args: list[str] | None = None, bxl_args: list[str] | None = None, env: dict[str, str] | None = None, **kwargs):
    """
    Creates a test target from a buck2 bxl script. BXL script must use "test" as entry
    point.

    Parameters:
        src: source path of BXL script, or an `export_file` target with the same
             name as the BXL script's path and `mode = "reference"`. Pass a
             target only if you need to use the same test script from `bxl_test`
             targets in different packages, otherwise the source path suffices.
        name: Name of the test target. If unspecified, use src as the name.
        buck_args: Arguments to `buck2 bxl` invocation for buck specifically.
            Common examples are `--config` flags and `---modifier` flags.
            Ex. buck_args = ["--config", "build.use_limited_hybrid=false"]
        bxl_args: Arguments to `buck2 bxl` invocation after `--`. These are
            arguments to bxl script specifically.
        env: Additional environment variables to pass to the test. These are
            merged with the internally-constructed env, with user-provided
            values taking precedence.
    """

    if not src.endswith(".bxl"):
        fail("`src` must end in '.bxl'. Found `{}` for `src`".format(src))

    if ":" not in src:
        # Need to include `name` to keep this target unique, in case there are multiple bxl_tests defined for same bxl file
        export_file_name = "{}.{}.export_file".format(src, name)
        export_file(name = export_file_name, src = src, mode = "reference")
        export_file_target = ":{}".format(export_file_name)

        bxl_main = "{}//{}/{}:test".format(native.get_cell_name(), native.package_name(), src)
        if not name:
            name = src
    else:
        export_file_target = src
        target_base, _, target_name = src.rpartition(":")
        bxl_main = "{}/{}:test".format(target_base, target_name)
        if not name:
            name = target_name

    merged_env = {
        "BXL_MAIN": bxl_main,
        # This env var is used to properly declare a dep on the src file.
        # I didn't use `resources` or `deps` because attaching to an env var makes debugging easier if needed.
        "_BXL_SRC": "$(location {})".format(export_file_target),
    }
    if bxl_args:
        merged_env["BXL_ARGS"] = " ".join(bxl_args)
    if buck_args:
        merged_env["BUCK_ARGS"] = " ".join(buck_args)
    if env:
        merged_env.update(env)

    buck2_e2e_test(
        name = name,
        env = merged_env,
        srcs = {"fbcode//buck2/tests/e2e_util:test_bxl_template.py": "test_bxl_template.py"},
        labels = ["bxl_test"] + (labels if labels else []),
        test_with_compiled_buck2 = False,
        test_with_deployed_buck2 = True,
        skip_deployed_buck2_version_dep = True,
        **kwargs,
    )
