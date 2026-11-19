# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# We only want to document the rules in the prelude, so have a module that
# only reexports those.
#
# NOTE: This file name is used to distinguish the Rules docs from API docs, if you change this file name,
# please update the rendering code `render_function` in `starlark-rust/starlark/src/docs/markdown.rs`

load("@prelude//:rules.bzl", _categorized_rules = "categorized_rules")

load_symbols({
    group_name: namespace(**{rule_name: namespace(**{rule_name: rule_obj}) for rule_name, rule_obj in group_rules.items()})
    for group_name, group_rules in _categorized_rules.items()
})
