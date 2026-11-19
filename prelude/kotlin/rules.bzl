# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":kotlin_library.bzl", kotlin_library_spec = "kotlin_library")
load(":kotlin_test.bzl", kotlin_test_spec = "kotlin_test")

specs = {
    "kotlin_library": kotlin_library_spec,
    "kotlin_test": kotlin_test_spec,
}

rules = make_rules(specs)

load_symbols(rules)
