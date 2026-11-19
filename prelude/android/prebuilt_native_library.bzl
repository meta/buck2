# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load(
    "@prelude//android:android_providers.bzl",
    "PrebuiltNativeLibraryDir",
    "merge_android_packageable_info",
)
load("@prelude//decls:common.bzl", "buck")

def prebuilt_native_library_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs.is_asset and ctx.attrs.has_wrap_script:
        fail("Cannot use `is_asset` and `has_wrap_script` in the same rule")

    prebuilt_native_library_dir = PrebuiltNativeLibraryDir(
        raw_target = ctx.label.raw_target(),
        dir = ctx.attrs.native_libs,
        for_primary_apk = ctx.attrs.has_wrap_script,
        is_asset = ctx.attrs.is_asset,
    )
    android_packageable_info = merge_android_packageable_info(
        ctx.label,
        ctx.actions,
        ctx.attrs.deps,
        prebuilt_native_library_dir = prebuilt_native_library_dir,
    )
    return [
        # Buck1 copies the input directory and returns it as the output path. We don't
        # copy; we could just return the input directory itself as the output path, but
        # we're avoiding that (due to potential confusion from the output path being an
        # input directory) until we have an actual need for prebuilt_native_library
        # having an output path.
        DefaultInfo(),
        android_packageable_info,
    ]

prebuilt_native_library = prelude_rule(
    name = "prebuilt_native_library",
    impl = prebuilt_native_library_impl,
    docs = """
        A `prebuilt_native_library()` rule is used to bundle native libraries
        (i.e., `.so` files) for Android.
    """,
    examples = """
        Most of the time, a `prebuilt_native_library` is private to the `android_library()` that uses it:

        ```
        prebuilt_native_library(
          name = 'native_libs',
          native_libs = 'libs',
        )

        android_library(
          name = 'my_lib',
          srcs = glob(['*.java']),
          deps = [
            ':native_libs',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "native_libs": attrs.source(
                allow_directory = True,
                doc = """
                Path to a directory containing native libraries.
                 This directory has subdirectories for different architectures,
                 such as `armeabi` and `armeabi-v7a`.
            """,
            ),
            "is_asset": attrs.bool(
                default = False,
                doc = """
                Normally native shared objects end up in a directory in the root of the APK
                 named `lib/`. If this parameter is set to `True`, then
                 these objects are placed in `assets/lib/`. Placing shared objects in
                 a non-standard location prevents Android from extracting them to the device's
                 internal storage.
            """,
            ),
            "has_wrap_script": attrs.bool(
                default = False,
                doc = """
                When using an exopackage, if this
                 parameter is set to `True`, then the libraries for this rule are
                 included in the primary APK even if native libraries would otherwise not be
                 placed in it. This is intended for a native library directory that contains a
                 [wrap.sh](https://developer.android.com/ndk/guides/wrap-script)
                 script, which must be included in the primary APK to take effect. Only one
                 of `is_asset` and `has_wrap_script` can be set for a rule.
            """,
            ),
            "deps": attrs.list(attrs.dep(), default = []),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
