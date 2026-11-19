# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//debugging:labels.bzl", "DBG_INFO_PYTHON_REQUIRES_SECONDARY_NATIVE_DEBUGINFO")

def _ensure_default_info(ctx: bxl.Context, default_info: DefaultInfo) -> None:
    ctx.output.ensure_multiple(cmd_args(default_info.default_outputs, hidden = default_info.other_outputs))

# This function ensures that certain subtargets are materialized if they exist. This is used by fdb.
def ensure_subtargets(ctx: bxl.Context, target: bxl.ConfiguredTargetNode):
    providers = ctx.analysis(target).providers()
    subtargets = providers[DefaultInfo].sub_targets
    labels = target.get_attr("labels") if target.get_attr("labels") else []

    # split dwarf targets have ["dwp"] subtargets. this function ensures that all dwp files are materialized
    if "dwp" in subtargets:
        _ensure_default_info(ctx, subtargets["dwp"][DefaultInfo])

    # Build modes whose toolchain sets `materialize_external_debug_info = False`
    # don't materialize unpacked split-dwarf debug info (.dwo) on regular
    # builds; materialize it here so debugging works without relying on dwp.
    if "debuginfo" in subtargets:
        _ensure_default_info(ctx, subtargets["debuginfo"][DefaultInfo])

    if (
        DBG_INFO_PYTHON_REQUIRES_SECONDARY_NATIVE_DEBUGINFO in labels
        and "native-executable" in subtargets
        and "dwp" in subtargets["native-executable"][DefaultInfo].sub_targets
    ):
        _ensure_default_info(ctx, subtargets["native-executable"][DefaultInfo].sub_targets["dwp"][DefaultInfo])

    # ios test targets need to materialize "dwarf-and-dsym" subtarget in order for debugging to work properly.
    if "dwarf-and-dsym" in subtargets:
        _ensure_default_info(ctx, subtargets["dwarf-and-dsym"][DefaultInfo])
