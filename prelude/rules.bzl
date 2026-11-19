# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# The rules the prelude provides, as `rule` objects. `native.bzl` exposes them as
# `native.<name>`, and they can also be loaded from this module by name.
#
# See `prelude_rule.bzl` for how rules are declared and why the `rule` objects
# are created in the per-language registries rather than here.

load("@prelude//:core_rules.bzl", _core_rules = "rules", _core_specs = "specs", _shell_rules = "shell_rules", _shell_specs = "shell_specs")
load("@prelude//:prelude_rule.bzl", "make_rule")
load("@prelude//android:rules.bzl", _android_rules = "rules", _android_specs = "specs")
load("@prelude//apple:rules.bzl", _apple_rules = "rules", _apple_specs = "specs")
load("@prelude//configurations:rules.bzl", _configuration_rules = "rules", _configuration_specs = "specs")
load("@prelude//csharp:rules.bzl", _dotnet_rules = "rules", _dotnet_specs = "specs")
load("@prelude//cxx:rules.bzl", _cxx_rules = "rules", _cxx_specs = "specs")
load("@prelude//erlang:rules.bzl", _erlang_rules = "rules", _erlang_specs = "specs")
load("@prelude//git:rules.bzl", _git_rules = "rules", _git_specs = "specs")
load("@prelude//go:rules.bzl", _go_rules = "rules", _go_specs = "specs")
load("@prelude//haskell:rules.bzl", _haskell_rules = "rules", _haskell_specs = "specs")
load("@prelude//java:rules.bzl", _java_rules = "rules", _java_specs = "specs")
load("@prelude//js:rules.bzl", _js_rules = "rules", _js_specs = "specs")
load("@prelude//julia:rules.bzl", _julia_rules = "rules", _julia_specs = "specs")
load("@prelude//kotlin:rules.bzl", _kotlin_rules = "rules", _kotlin_specs = "specs")
load("@prelude//lua:rules.bzl", _lua_rules = "rules", _lua_specs = "specs")
load("@prelude//matlab:rules.bzl", _matlab_rules = "rules", _matlab_specs = "specs")
load("@prelude//nodejs:rules.bzl", _nodejs_rules = "rules", _nodejs_specs = "specs")
load("@prelude//ocaml:rules.bzl", _ocaml_rules = "rules", _ocaml_specs = "specs")
load("@prelude//python:rules.bzl", _python_rules = "rules", _python_specs = "specs")
load("@prelude//python/cython:rules.bzl", _cython_rules = "rules", _cython_specs = "specs")
load("@prelude//rust:rules.bzl", _rust_rules = "rules", _rust_specs = "specs")

# Rules grouped by the category they are documented under.
categorized_rules = {
    ".NET": _dotnet_rules,
    "android": _android_rules,
    "apple": _apple_rules,
    "core": _core_rules | _configuration_rules,
    "cxx": _cxx_rules,
    "cython": _cython_rules,
    "erlang": _erlang_rules,
    "git": _git_rules,
    "go": _go_rules,
    "haskell": _haskell_rules,
    "java": _java_rules,
    "js": _js_rules,
    "julia": _julia_rules,
    "kotlin": _kotlin_rules,
    "lua": _lua_rules,
    "matlab": _matlab_rules,
    "nodejs": _nodejs_rules,
    "ocaml": _ocaml_rules,
    "python": _python_rules,
    "rust": _rust_rules,
    "shell": _shell_rules,
}

_categorized_rule_specs = {
    ".NET": _dotnet_specs,
    "android": _android_specs,
    "apple": _apple_specs,
    "core": _core_specs | _configuration_specs,
    "cxx": _cxx_specs,
    "cython": _cython_specs,
    "erlang": _erlang_specs,
    "git": _git_specs,
    "go": _go_specs,
    "haskell": _haskell_specs,
    "java": _java_specs,
    "js": _js_specs,
    "julia": _julia_specs,
    "kotlin": _kotlin_specs,
    "lua": _lua_specs,
    "matlab": _matlab_specs,
    "nodejs": _nodejs_specs,
    "ocaml": _ocaml_specs,
    "python": _python_specs,
    "rust": _rust_specs,
    "shell": _shell_specs,
}

def _flatten(categorized: dict[str, dict[str, typing.Any]]) -> dict[str, typing.Any]:
    flat = {}
    for category, by_name in categorized.items():
        for name, value in by_name.items():
            if name in flat:
                fail("Rule `{}` is declared in more than one category (`{}`)".format(name, category))
            flat[name] = value
    return flat

rules = _flatten(categorized_rules)

_rule_specs = _flatten(_categorized_rule_specs)

# The rules are accessed by doing module.name, so we have to put them on the correct module.
load_symbols(rules)

# TODO(akrieger): Remove this and instead refactor to allow impl bzl files to export attrs.
def clone_rule(rule: str, extra_attrs: dict[str, typing.Any] = dict(), impl_override = None, **kwargs):
    if not rule in _rule_specs:
        fail("Tried clone rule {} which does not exist".format(rule))
    return make_rule(_rule_specs[rule], extra_attrs, impl_override, **kwargs)
