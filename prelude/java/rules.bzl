# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load("@prelude//java/plugins:java_annotation_processor.bzl", java_annotation_processor_spec = "java_annotation_processor")
load("@prelude//java/plugins:java_plugin.bzl", java_plugin_spec = "java_plugin")
load(":gwt_binary.bzl", gwt_binary_spec = "gwt_binary")
load(":jar_genrule.bzl", jar_genrule_spec = "jar_genrule")
load(":java_binary.bzl", java_binary_spec = "java_binary")
load(":java_library.bzl", java_library_spec = "java_library")
load(":java_test.bzl", java_test_spec = "java_test")
load(":prebuilt_jar.bzl", prebuilt_jar_spec = "prebuilt_jar")

specs = {
    "gwt_binary": gwt_binary_spec,
    "jar_genrule": jar_genrule_spec,
    "java_annotation_processor": java_annotation_processor_spec,
    "java_binary": java_binary_spec,
    "java_library": java_library_spec,
    "java_plugin": java_plugin_spec,
    "java_test": java_test_spec,
    "prebuilt_jar": prebuilt_jar_spec,
}

rules = make_rules(specs)

load_symbols(rules)
