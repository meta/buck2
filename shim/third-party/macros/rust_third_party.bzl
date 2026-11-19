# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//rust:cargo_package.bzl", "cargo", "get_reindeer_platforms")
load("@prelude//utils:selects.bzl", "selects")

def _target_constraints(platforms, kwargs):
    target_compatible_with = kwargs.pop("target_compatible_with", [])

    if kwargs.get("proc_macro", False) or len(platforms) == 0:
        return target_compatible_with
    else:
        return selects.apply(
            get_reindeer_platforms(),
            lambda p: target_compatible_with if p in platforms else ["prelude//:none"],
        )

def third_party_rust_alias(name, platforms = [], **kwargs):
    target_compatible_with = _target_constraints(platforms, kwargs)

    # @lint-ignore BUCKLINT: avoid "Direct usage of native rules is not allowed."
    native.alias(name = name, target_compatible_with = target_compatible_with, **kwargs)

def third_party_rust_library(name, platform = {}, **kwargs):
    target_compatible_with = _target_constraints(platform, kwargs)

    cargo.rust_library(name = name, platform = platform, target_compatible_with = target_compatible_with, **kwargs)

def third_party_rust_binary(name, platform = {}, **kwargs):
    target_compatible_with = _target_constraints(platform, kwargs)

    cargo.rust_binary(name = name, platform = platform, target_compatible_with = target_compatible_with, **kwargs)

def third_party_rust_cxx_library(name, platform = {}, **kwargs):
    target_compatible_with = _target_constraints(platform, kwargs)

    # @lint-ignore BUCKLINT: avoid "Direct usage of native rules is not allowed."
    native.cxx_library(name = name, target_compatible_with = target_compatible_with, **kwargs)

def third_party_rust_prebuilt_cxx_library(name, platform = {}, **kwargs):
    # FIXME: This should probably be a fixup.toml, but it currently can't be expressed.
    # The windows-sys crate does -lwindows to find windows. We pass libwindows.a on the command line,
    # which resolves the symbols, but the linker still needs to "find" windows, so we also put its
    # directory on the link options.
    if name.endswith("libwindows.a"):
        kwargs["exported_linker_flags"] = ["-Lshim/third-party/rust/" + kwargs["static_lib"].rpartition("/")[0]]

    target_compatible_with = _target_constraints(platform, kwargs)

    # @lint-ignore BUCKLINT: avoid "Direct usage of native rules is not allowed."
    native.prebuilt_cxx_library(name = name, target_compatible_with = target_compatible_with, **kwargs)
