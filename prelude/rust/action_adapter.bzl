# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def adapt_rustc_action(ctx, command, adapter, prefix):
    """Optionally wrap the entire action, before rustc_action parses arguments.

    The declared RunInfo receives --sandbox-outputs=<JSON> followed by the
    original command. JSON v1 contains outputs (including diagnostics and hidden
    outputs) and views (currently empty). RUST_SANDBOX_METADATA names Buck's
    declared-input metadata. The adapter must execute the original command and
    propagate its exit status. Its runtime resources belong in its RunInfo.

    This is only an interception seam, not a sandbox or cache-admission proof.
    It does not wrap rustdoc, build scripts, linkers, or other target actions.
    With the flag absent/false the original command and run options are returned
    unchanged: no extra artifacts, inputs, environment, or metadata are created.
    """
    enabled = read_root_config("rust", "action_adapter_enabled", "false")
    if enabled not in ["true", "false"]:
        fail("rust.action_adapter_enabled must be true or false")
    if enabled == "false":
        return command, {}
    if adapter == None:
        fail("rust.action_adapter_enabled requires a declared rustc_action_adapter")
    manifest = ctx.actions.write_json(
        prefix + "-sandbox-outputs.json",
        {
            "version": 1,
            "outputs": [cmd_args(command.outputs, ignore_artifacts = True)],
            "views": [],
        },
    )
    return cmd_args(adapter, cmd_args(manifest, format = "--sandbox-outputs={}"), command), {
        "metadata_env_var": "RUST_SANDBOX_METADATA",
        "metadata_path": prefix + "-sandbox-inputs.json",
    }
