# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:buckconfig.bzl", "read_bool")

# Whether the build mode skips a test, as with `skip_on_mode_mac`.  Open source
# has no `mode/mac` or `mode/win`, so unless a project sets these configs, no
# rule is a no-op.
def is_noop_rule(rule_name, skip_on_mode_mac = False, skip_on_mode_win = False):
    _rule_name = rule_name  # @unused
    if skip_on_mode_mac and read_bool("fbcode", "mode_mac_enabled", default = False, required = False):
        return True
    if skip_on_mode_win and read_bool("fbcode", "mode_win_enabled", default = False, required = False):
        return True
    return False
