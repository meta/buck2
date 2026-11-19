# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":android_aar.bzl", android_aar_spec = "android_aar")
load(":android_apk.bzl", android_binary_spec = "android_binary")
load(":android_build_config.bzl", android_build_config_spec = "android_build_config")
load(":android_bundle.bzl", android_bundle_spec = "android_bundle")
load(":android_instrumentation_apk.bzl", android_instrumentation_apk_spec = "android_instrumentation_apk")
load(":android_instrumentation_test.bzl", android_instrumentation_test_spec = "android_instrumentation_test")
load(":android_library.bzl", android_library_spec = "android_library")
load(":android_manifest.bzl", android_manifest_spec = "android_manifest")
load(":android_prebuilt_aar.bzl", android_prebuilt_aar_spec = "android_prebuilt_aar")
load(":android_resource.bzl", android_resource_spec = "android_resource")
load(":apk_genrule.bzl", apk_genrule_spec = "apk_genrule")
load(":gen_aidl.bzl", gen_aidl_spec = "gen_aidl")
load(":keystore.bzl", keystore_spec = "keystore")
load(":ndk_toolchain.bzl", ndk_toolchain_spec = "ndk_toolchain")
load(":prebuilt_native_library.bzl", prebuilt_native_library_spec = "prebuilt_native_library")
load(":robolectric_test.bzl", robolectric_test_spec = "robolectric_test")
load(":voltron.bzl", android_app_modularity_spec = "android_app_modularity")

specs = {
    "android_aar": android_aar_spec,
    "android_app_modularity": android_app_modularity_spec,
    "android_binary": android_binary_spec,
    "android_build_config": android_build_config_spec,
    "android_bundle": android_bundle_spec,
    "android_instrumentation_apk": android_instrumentation_apk_spec,
    "android_instrumentation_test": android_instrumentation_test_spec,
    "android_library": android_library_spec,
    "android_manifest": android_manifest_spec,
    "android_prebuilt_aar": android_prebuilt_aar_spec,
    "android_resource": android_resource_spec,
    "apk_genrule": apk_genrule_spec,
    "gen_aidl": gen_aidl_spec,
    "keystore": keystore_spec,
    "ndk_toolchain": ndk_toolchain_spec,
    "prebuilt_native_library": prebuilt_native_library_spec,
    "robolectric_test": robolectric_test_spec,
}

rules = make_rules(specs)

load_symbols(rules)
