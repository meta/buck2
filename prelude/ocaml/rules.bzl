# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(
    ":ocaml.bzl",
    ocaml_binary_spec = "ocaml_binary",
    ocaml_library_spec = "ocaml_library",
    ocaml_object_spec = "ocaml_object",
    ocaml_shared_spec = "ocaml_shared",
    prebuilt_ocaml_library_spec = "prebuilt_ocaml_library",
)

specs = {
    "ocaml_binary": ocaml_binary_spec,
    "ocaml_library": ocaml_library_spec,
    "ocaml_object": ocaml_object_spec,
    "ocaml_shared": ocaml_shared_spec,
    "prebuilt_ocaml_library": prebuilt_ocaml_library_spec,
}

rules = make_rules(specs)

load_symbols(rules)
