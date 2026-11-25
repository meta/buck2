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
load("@prelude//user:all.bzl", _user_rules = "rules")

# The category that each rule of `prelude//user:all.bzl` is documented under.
# One that is not listed goes to `core`.
_USER_RULE_CATEGORIES = {
    "android_emulators": "android",
    "cxx_headers_bundle": "cxx",
    "cxx_toolchain_override": "cxx",
    "link_group_map": "cxx",
    "xcode_files": "apple",
}

def _documented_rules() -> dict[str, dict[str, typing.Any]]:
    documented = {category: dict(rules) for category, rules in _categorized_rules.items()}
    names = {name: True for rules in documented.values() for name in rules}
    for name, rule in _user_rules.items():
        if name in names:
            fail("`prelude//user:all.bzl` and `prelude//:rules.bzl` both define the rule `{}`".format(name))
        documented[_USER_RULE_CATEGORIES.get(name, "core")][name] = rule
    return documented

load_symbols({
    group_name: namespace(**{rule_name: namespace(**{rule_name: rule_obj}) for rule_name, rule_obj in group_rules.items()})
    for group_name, group_rules in _documented_rules().items()
})
