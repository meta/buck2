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
    "AndroidManifestInfo",
    "ManifestTSet",  # @unused Used as type
    "merge_android_packageable_info",
)
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:voltron.bzl", "ROOT_MODULE")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//utils:argfile.bzl", "argfile")

def android_manifest_impl(ctx: AnalysisContext) -> list[Provider]:
    output, merge_report = generate_android_manifest(
        ctx,
        ctx.attrs._android_toolchain[AndroidToolchainInfo].generate_manifest[RunInfo],
        ctx.attrs.skeleton,
        ROOT_MODULE,
        _get_manifests_from_deps(ctx),
        {},
    )

    return [
        AndroidManifestInfo(manifest = output, merge_report = merge_report),
        DefaultInfo(default_output = output, other_outputs = [merge_report]),
    ]

def generate_android_manifest(
    ctx: AnalysisContext,
    generate_manifest: RunInfo,
    manifest_skeleton: Artifact,
    module_name: str,
    manifests: [ManifestTSet, list[Artifact], None],
    placeholder_entries: dict,
) -> (Artifact, Artifact):
    generate_manifest_cmd = cmd_args(generate_manifest)
    generate_manifest_cmd.add([
        "--skeleton-manifest",
        manifest_skeleton,
        "--module-name",
        module_name,
    ])

    if not manifests:
        manifests = []
    elif isinstance(manifests, TransitiveSet):
        # Manifests are usually resolved only at the binary level, so resolving
        # projection nodes throughout the dependency graph is wasteful.
        manifests = [entry.manifest for entry in manifests.traverse(ordering = "topological")]

    library_manifest_paths_file = argfile(actions = ctx.actions, name = "{}/library_manifest_paths_file".format(module_name), args = manifests)

    generate_manifest_cmd.add(["--library-manifests-list", library_manifest_paths_file])

    placeholder_entries_args = cmd_args()
    for key, val in placeholder_entries.items():
        placeholder_entries_args.add(cmd_args(str(key), str(val), delimiter = " "))
    placeholder_entries_file = ctx.actions.write("{}/placeholder_entries_file".format(module_name), placeholder_entries_args, has_content_based_path = False)

    generate_manifest_cmd.add(["--placeholder-entries-list", placeholder_entries_file])

    output = ctx.actions.declare_output("{}/AndroidManifest.xml".format(module_name), has_content_based_path = False)
    merge_report = ctx.actions.declare_output("{}/merge-report.txt".format(module_name), has_content_based_path = False)
    preprocess_log = ctx.actions.declare_output("{}/preprocess-log.txt".format(module_name), has_content_based_path = False)
    generate_manifest_cmd.add([
        "--output",
        output.as_output(),
        "--merge-report",
        merge_report.as_output(),
        "--preprocess-log",
        preprocess_log.as_output(),
    ])

    ctx.actions.run(generate_manifest_cmd, category = "generate_manifest", identifier = module_name)

    return (output, merge_report)

def _get_manifests_from_deps(ctx: AnalysisContext) -> [ManifestTSet, None]:
    if len(ctx.attrs.deps) == 0:
        return None

    android_packageable_info = merge_android_packageable_info(ctx.label, ctx.actions, ctx.attrs.deps)
    return android_packageable_info.manifests

android_manifest = prelude_rule(
    name = "android_manifest",
    impl = android_manifest_impl,
    docs = """
        An `android_manifest()` rule is used to generate an
         [Android
        Manifest](http://developer.android.com/guide/topics/manifest/manifest-intro.html) to be used by `android_binary()` and `android_aar()` rules. This
        rule takes a skeleton manifest, and merges it with manifests found in any deps.
    """,
    examples = """
        Here's an example of an `android_manifest()` that has no deps.

        ```
        android_manifest(
          name = 'my-manifest',
          skeleton = 'AndroidManifestSkeleton.xml',
        )
        ```

         This is what `AndroidManifestSkeleton.xml` looks like.

        ```
        <?xml version="1.0" encoding="utf-8"?>
        <manifest xmlns:android="http://schemas.android.com/apk/res/android"
                  package="com.example"
                  android:versionCode="1"
                  android:versionName="1.0">

            <uses-sdk targetSdkVersion="19" minSdkVersion="17"/>
            <application
                android:label="@string/app_name"
                android:icon="@drawable/ic_launcher">
                <activity
                    android:name="MyActivity"
                    android:label="@string/app_name">
                    <intent-filter>
                        <action android:name="android.intent.action.MAIN"/>
                        <category android:name="android.intent.category.LAUNCHER"/>
                    </intent-filter>
                </activity>
            </application>
        </manifest>
        ```

         You could also use a `genrule()` to generate the manifest file and reference the
         `build target` in the `skeleton` argument.
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "skeleton": attrs.source(
                doc = """
                Either a `build target` or a path to a file representing the manifest that
                 will be merged with any manifests associated with this rule's `deps`.
            """,
            ),
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                A collection of dependencies that includes android\\_library rules. The manifest files of the
                 `android_library()` rules will be filtered out to become dependent source files for
                 the manifest.
            """,
            ),
            "_android_toolchain": toolchains_common.android(),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
