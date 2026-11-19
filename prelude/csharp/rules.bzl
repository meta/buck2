# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":csharp.bzl", csharp_library_spec = "csharp_library", prebuilt_dotnet_library_spec = "prebuilt_dotnet_library")

specs = {
    "csharp_library": csharp_library_spec,
    "prebuilt_dotnet_library": prebuilt_dotnet_library_spec,
}

rules = make_rules(specs)

load_symbols(rules)
