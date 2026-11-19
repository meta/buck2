# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":js_bundle.bzl", js_bundle_spec = "js_bundle")
load(":js_bundle_genrule.bzl", js_bundle_genrule_spec = "js_bundle_genrule")
load(":js_library.bzl", js_library_spec = "js_library")

specs = {
    "js_bundle": js_bundle_spec,
    "js_bundle_genrule": js_bundle_genrule_spec,
    "js_library": js_library_spec,
}

rules = make_rules(specs)

load_symbols(rules)
