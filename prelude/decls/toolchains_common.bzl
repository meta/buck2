# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//android:android_toolchain.bzl", "AidlToolchainInfo", "AndroidPlatformInfo", "AndroidToolchainInfo")
load("@prelude//csharp:toolchain.bzl", "CSharpToolchainInfo")
load("@prelude//go:toolchain.bzl", "GoToolchainInfo")
load("@prelude//go_bootstrap:toolchain.bzl", "GoBootstrapToolchainInfo")
load("@prelude//haskell:toolchain.bzl", "HaskellPlatformInfo", "HaskellToolchainInfo")
load("@prelude//java:dex_toolchain.bzl", "DexToolchainInfo")
load(
    "@prelude//java:java_toolchain.bzl",
    "JavaPlatformInfo",
    "JavaTestToolchainInfo",
    "JavaToolchainInfo",
    "PrebuiltJarToolchainInfo",
)
load(
    "@prelude//kotlin:kotlin_toolchain.bzl",
    "KotlinToolchainInfo",
)
load("@prelude//nodejs:nodejs_toolchain.bzl", "NodejsToolchainInfo")
load("@prelude//python:python_wheel_toolchain.bzl", "PythonWheelToolchainInfo")
load("@prelude//python:toolchain.bzl", "PythonPlatformInfo", "PythonToolchainInfo")
load("@prelude//python/cython:cython_toolchain.bzl", "CythonToolchainInfo")
load("@prelude//python_bootstrap:toolchain.bzl", "PythonBootstrapToolchainInfo")
load("@prelude//rust:rust_toolchain.bzl", "RustToolchainInfo")
load("@prelude//tests:remote_test_execution_toolchain.bzl", "RemoteTestExecutionToolchainInfo")
load("@prelude//tests:test_listing.bzl", "TestListingInfo")
load("@prelude//tests:test_toolchain.bzl", "TestToolchainInfo")
load("@prelude//zip_file:zip_file_toolchain.bzl", "ZipFileToolchainInfo")

def _toolchain(lang: str, providers: list[typing.Any], *, default: typing.Any = None, exec_group: [ExecGroup, None] = None) -> Attr:
    return attrs.toolchain_dep(default = default or ("toolchains//:" + lang), providers = providers, exec_group = exec_group)

def _android_toolchain():
    return _toolchain("android", [AndroidToolchainInfo, AndroidPlatformInfo, TestListingInfo])

def _android_aidl_toolchain():
    return _toolchain("android_aidl", [AidlToolchainInfo])

def _csharp_toolchain():
    return _toolchain("csharp", [CSharpToolchainInfo])

def _cython_toolchain():
    return _toolchain("cython", [CythonToolchainInfo])

def _cxx_toolchain():
    # `CxxToolchainInfo, CxxPlatformInfo`, but python doesn't require it
    lang = "cxx"

    # Portions of the macro layers use the `default_deps` attribute to set this
    # to the `:cxx_no_default_deps` option. If a targets has within_view checks
    # that don't list `toolchains//:` they will experience an error. We can avoid
    # this by ensuring the `within_deps` checks inside buck see either of the
    # toolchains below as a possible default value, even though one of them is
    # never able to be selected in this expression.
    return _toolchain(lang, [], default = select({"DEFAULT": "toolchains//:" + lang, "config//:none": "toolchains//:cxx_no_default_deps"}))

def _dex_toolchain():
    return _toolchain("dex", [DexToolchainInfo])

def _go_toolchain():
    return _toolchain("go", [GoToolchainInfo])

def _go_bootstrap_toolchain():
    return _toolchain("go_bootstrap", [GoBootstrapToolchainInfo])

def _haskell_toolchain():
    return _toolchain("haskell", [HaskellToolchainInfo, HaskellPlatformInfo])

def _java_toolchain():
    return _toolchain("java", [JavaToolchainInfo, JavaPlatformInfo])

def _java_bootstrap_toolchain():
    return _toolchain("java_bootstrap", [JavaToolchainInfo, JavaPlatformInfo])

def _java_for_android_toolchain():
    return _toolchain("java_for_android", [JavaToolchainInfo, JavaPlatformInfo])

def _java_for_host_test_toolchain():
    return _toolchain("java_for_host_test", [JavaToolchainInfo, JavaPlatformInfo])

def _java_test_toolchain():
    return _toolchain("java_test", [JavaTestToolchainInfo, TestListingInfo])

def _kotlin_toolchain():
    return _toolchain("kotlin", [KotlinToolchainInfo])

def _kotlin_for_android_toolchain():
    return _toolchain("kotlin_for_android", [KotlinToolchainInfo])

def _nodejs_toolchain():
    return _toolchain("nodejs", [NodejsToolchainInfo])

def _prebuilt_jar_toolchain():
    # Override is allowed for bootstrapping prebuilt jar toolchains
    return _toolchain("prebuilt_jar", [PrebuiltJarToolchainInfo])

def _python_toolchain():
    return _toolchain("python", [PythonToolchainInfo, PythonPlatformInfo])

def _python_bootstrap_toolchain():
    return _toolchain("python_bootstrap", [PythonBootstrapToolchainInfo])

def _python_wheel_toolchain():
    return _toolchain("python_wheel", [PythonWheelToolchainInfo])

def _rust_toolchain():
    return _toolchain("rust", [RustToolchainInfo])

def _zip_file_toolchain():
    return _toolchain("zip_file", [ZipFileToolchainInfo])

def _remote_test_execution_toolchain():
    return _toolchain("remote_test_execution", [RemoteTestExecutionToolchainInfo])

def _test_toolchain():
    # Test rules with named execution groups, such as `apple_test`, must name the group of every
    # toolchain dep.
    return _toolchain("test", [TestToolchainInfo], exec_group = DefaultExecGroup)

toolchains_common = struct(
    android = _android_toolchain,
    android_aidl = _android_aidl_toolchain,
    csharp = _csharp_toolchain,
    cxx = _cxx_toolchain,
    cython = _cython_toolchain,
    dex = _dex_toolchain,
    go = _go_toolchain,
    go_bootstrap = _go_bootstrap_toolchain,
    haskell = _haskell_toolchain,
    java = _java_toolchain,
    java_bootstrap = _java_bootstrap_toolchain,
    java_for_android = _java_for_android_toolchain,
    java_for_host_test = _java_for_host_test_toolchain,
    java_test = _java_test_toolchain,
    kotlin = _kotlin_toolchain,
    kotlin_for_android = _kotlin_for_android_toolchain,
    nodejs = _nodejs_toolchain,
    prebuilt_jar = _prebuilt_jar_toolchain,
    python = _python_toolchain,
    python_bootstrap = _python_bootstrap_toolchain,
    python_wheel = _python_wheel_toolchain,
    test_toolchain = _test_toolchain,
    rust = _rust_toolchain,
    zip_file = _zip_file_toolchain,
    remote_test_execution = _remote_test_execution_toolchain,
)
