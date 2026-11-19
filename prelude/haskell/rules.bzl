# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":haskell.bzl", haskell_binary_spec = "haskell_binary", haskell_library_spec = "haskell_library", haskell_prebuilt_library_spec = "haskell_prebuilt_library")
load(":haskell_ghci.bzl", haskell_ghci_spec = "haskell_ghci")
load(":haskell_haddock.bzl", haskell_haddock_spec = "haskell_haddock")
load(":haskell_ide.bzl", haskell_ide_spec = "haskell_ide")

specs = {
    "haskell_binary": haskell_binary_spec,
    "haskell_ghci": haskell_ghci_spec,
    "haskell_haddock": haskell_haddock_spec,
    "haskell_ide": haskell_ide_spec,
    "haskell_library": haskell_library_spec,
    "haskell_prebuilt_library": haskell_prebuilt_library_spec,
}

rules = make_rules(specs)

load_symbols(rules)
