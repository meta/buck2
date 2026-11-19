# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//android:android_providers.bzl", "KeystoreInfo")  # @unused used as type
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")

def derive_universal_apk(ctx: AnalysisContext, android_toolchain: AndroidToolchainInfo, app_bundle: Artifact, keystore: [KeystoreInfo, None]) -> Artifact:
    output_apk = ctx.actions.declare_output("universal.apk", has_content_based_path = False)

    bundle_apks_builder_args = cmd_args([
        android_toolchain.bundle_apks_builder[RunInfo],
        "--input-bundle",
        app_bundle,
        "--p7zip",
        android_toolchain.p7zip,
        "--aapt2",
        android_toolchain.aapt2,
        "--zipalign",
        android_toolchain.zipalign[RunInfo],
        "--output-apk",
        output_apk.as_output(),
    ])

    if keystore:
        bundle_apks_builder_args.add(
            cmd_args([
                "--keystore",
                keystore.store,
                "--keystore-properties",
                keystore.properties,
            ])
        )

    ctx.actions.run(bundle_apks_builder_args, category = "bundle_build", identifier = "build_universal_apk")

    return output_apk
