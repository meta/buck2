# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//tests:test_toolchain.bzl", "noop_test_toolchain")
load("@prelude//toolchains:cxx.bzl", "system_cxx_toolchain")
load("@prelude//toolchains:genrule.bzl", "system_genrule_toolchain")
load("@prelude//toolchains:go.bzl", "system_go_bootstrap_toolchain", "system_go_toolchain")
load("@prelude//toolchains:haskell.bzl", "system_haskell_toolchain")
load("@prelude//toolchains:ocaml.bzl", "system_ocaml_toolchain")
load("@prelude//toolchains:python.bzl", "system_python_bootstrap_toolchain", "system_python_toolchain")
load("@prelude//toolchains:remote_test_execution.bzl", "remote_test_execution_toolchain")
load("@prelude//toolchains:rust.bzl", "system_rust_toolchain")

oncall("open_source")

configuration_alias(
    name = "none",
    actual = "prelude//:none",
    visibility = ["PUBLIC"],
)

system_cxx_toolchain(
    name = "cxx",
    compiler = select({
        "DEFAULT": "gcc",
        "ovr_config//os:windows": None,
        "ovr_config//os:windows-clang": "clang-cl",
    }),
    # `compiler_type = "gcc"` is required in addition to the compiler
    # binary names. The prelude gates many flags on `compiler_type`
    # rather than the binary name (e.g. `prelude/cxx/compiler.bzl` adds
    # `-Xclang -fdebug-compilation-dir ... -fcolor-diagnostics` only when
    # `compiler_type in ["clang", ...]`). Without this, gcc is invoked
    # with clang-only flags and rejects them.
    compiler_type = select({
        "DEFAULT": "gcc",
        "ovr_config//os:windows": "windows",
        "ovr_config//os:windows-clang": "clang_cl",
    }),
    cxx_compiler = select({
        "DEFAULT": "g++",
        "ovr_config//os:windows": None,
    }),
    cxx_flags = select({
        "DEFAULT": ["-std=c++20"],
        "ovr_config//os:windows": ["/std:c++20", "/EHsc"],
        "ovr_config//os:windows-clang": ["/std:c++20", "/EHsc"],
    }),
    # The sandcastle OSS-bootstrap environment has `gcc`/`g++` on PATH but
    # not `clang`/`clang++` or `lld`, which are the defaults pulled in by
    # `prelude//toolchains/cxx/clang:path_clang_tools`. Using `g++` as the
    # linker also causes `prelude/toolchains/cxx.bzl` to drop its
    # automatic `-fuse-ld=lld` link flag (see the g++-gated branch there),
    # so we don't need lld either.
    linker = select({
        "DEFAULT": "g++",
        "ovr_config//os:windows": None,
    }),
    visibility = ["PUBLIC"],
)

system_genrule_toolchain(
    name = "genrule",
    visibility = ["PUBLIC"],
)

system_go_toolchain(
    name = "go",
    visibility = ["PUBLIC"],
)

system_go_bootstrap_toolchain(
    name = "go_bootstrap",
    visibility = ["PUBLIC"],
)

system_haskell_toolchain(
    name = "haskell",
    visibility = ["PUBLIC"],
)

system_ocaml_toolchain(
    name = "ocaml",
    visibility = ["PUBLIC"],
)

system_python_toolchain(
    name = "python",
    visibility = ["PUBLIC"],
)

system_python_bootstrap_toolchain(
    name = "python_bootstrap",
    visibility = ["PUBLIC"],
)

system_rust_toolchain(
    name = "rust",
    default_edition = "2024",
    visibility = ["PUBLIC"],
)

remote_test_execution_toolchain(
    name = "remote_test_execution",
    visibility = ["PUBLIC"],
)

noop_test_toolchain(
    name = "test",
    visibility = ["PUBLIC"],
)
