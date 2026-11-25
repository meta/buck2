# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Module containing java macros."""

load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")
load("@prelude//:native.bzl", "native")
# @oss-disable[end= ]: load("@prelude//android/meta_only:android_build_tools_cas_artifact.bzl", "android_build_tools_cas_artifact")
load("@prelude//toolchains/android/tools/build_rules:fb_native.bzl", "fb_native")
load("@prelude//toolchains/android/tools/build_rules:utils.bzl", "add_os_labels")
load("@prelude//utils:selects.bzl", "selects")

SIGNED_JAR_BLOCKLIST = [
    "META-INF/.*\\.DSA",
    "META-INF/.*\\.SF",
    "META-INF/.*\\.RSA",
]

OPEN_JDK_COMPILER_ARGS = [
    "--add-exports=jdk.compiler/com.sun.tools.javac.code=ALL-UNNAMED",
    "--add-exports=jdk.compiler/com.sun.tools.javac.comp=ALL-UNNAMED",
    "--add-exports=jdk.compiler/com.sun.tools.javac.file=ALL-UNNAMED",
    "--add-exports=jdk.compiler/com.sun.tools.javac.jvm=ALL-UNNAMED",
    "--add-exports=jdk.compiler/com.sun.tools.javac.model=ALL-UNNAMED",
    "--add-exports=jdk.compiler/com.sun.tools.javac.parser=ALL-UNNAMED",
    "--add-exports=jdk.compiler/com.sun.tools.javac.processing=ALL-UNNAMED",
    "--add-exports=jdk.compiler/com.sun.tools.javac.tree=ALL-UNNAMED",
    "--add-opens=jdk.compiler/com.sun.tools.javac.api=ALL-UNNAMED",
    "--add-opens=jdk.compiler/com.sun.tools.javac.code=ALL-UNNAMED",
    "--add-opens=jdk.compiler/com.sun.tools.javac.jvm=ALL-UNNAMED",
    "--add-opens=jdk.compiler/com.sun.tools.javac.main=ALL-UNNAMED",
    "--add-opens=jdk.compiler/com.sun.tools.javac.processing=ALL-UNNAMED",
    "--add-opens=jdk.compiler/com.sun.tools.javac.util=ALL-UNNAMED",
]

_RUN_AS_BUNDLE_LABEL = "run_as_bundle"

_FDB_DEBUG_LABEL = "fdb:target:android"

def _maybe_add_java_version(**kwargs):
    if "source" not in kwargs and "target" not in kwargs and "java_version" not in kwargs:
        kwargs["java_version"] = "11"
    return kwargs

def _add_labels(**kwargs):
    if "labels" not in kwargs:
        kwargs["labels"] = []
    kwargs["labels"] += ["wrapped_with_buck_java_rules", "pfh:Infra"]
    return kwargs

def _set_buck2_java_toolchain(**kwargs):
    # Default to the bootstrap toolchain, but let callers opt into a different one (e.g. a
    # non-bootstrap toolchain when the binary packages native code, which the bootstrap fat-jar
    # tooling does not support).
    if "_java_toolchain" not in kwargs:
        kwargs["_java_toolchain"] = "toolchains//:java_bootstrap"
    return kwargs

def _set_buck2_kotlin_toolchain(**kwargs):
    kwargs["_kotlin_toolchain"] = "toolchains//:kotlin_bootstrap"
    return kwargs

def _set_buck2_dex_toolchain(**kwargs):
    # Override dex toolchain to avoid dependency cycles in unconfigured graph
    dex_toolchain = kwargs.pop("_dex_toolchain", None)
    kwargs["_dex_toolchain"] = dex_toolchain or select({
        "DEFAULT": "toolchains//:empty_dex",
        "config//os/constraints:android": "toolchains//:dex",
    })
    return kwargs

def _set_versioned_java_srcs(**kwargs):
    if not kwargs.pop("versioned_java_srcs", False):
        return kwargs
    java_version = select({
        "DEFAULT": native.read_config("java", "buck2_java_version", "21"),
        # @oss-disable[end= ]: "fbsource//third-party/toolchains/jdk:constraint-setting-version[11]": "11",
        # @oss-disable[end= ]: "fbsource//third-party/toolchains/jdk:constraint-setting-version[17]": "17",
        # @oss-disable[end= ]: "fbsource//third-party/toolchains/jdk:constraint-setting-version[21]": "21",
    })
    versioned_srcs = selects.apply(
        java_version,
        lambda value: kwargs.get("srcs", []) + native.glob(["java{}/*.java".format(value)]),
    )
    kwargs["srcs"] = versioned_srcs
    return kwargs

def _add_kotlin_deps(**kwargs):
    kwargs["deps"] = kwargs.pop("deps", []) + [
        "prelude//toolchains/android/third-party:kotlin-annotations",
        "prelude//toolchains/android/third-party:kotlin-stdlib",
    ]
    return kwargs

def buck_kotlin_library(name, **kwargs):
    kwargs = _maybe_add_java_version(**kwargs)
    kwargs = _set_buck2_java_toolchain(**kwargs)
    kwargs = _set_buck2_kotlin_toolchain(**kwargs)
    kwargs = _set_buck2_dex_toolchain(**kwargs)
    kwargs = _add_kotlin_deps(**kwargs)
    return fb_native.kotlin_library(name = name, **kwargs)

def buck_java_library(name, **kwargs):
    kwargs = _add_labels(**kwargs)
    kwargs = _maybe_add_java_version(**kwargs)
    kwargs = _set_buck2_java_toolchain(**kwargs)
    kwargs = _set_buck2_dex_toolchain(**kwargs)
    kwargs = _set_versioned_java_srcs(**kwargs)
    return fb_native.java_library(name = name, **kwargs)

def buck_java_binary(name, **kwargs):
    kwargs = _add_labels(**kwargs)
    kwargs = _set_buck2_java_toolchain(**kwargs)
    java_args = kwargs["java_args_for_run_info"] if "java_args_for_run_info" in kwargs else []

    # Directs the VM to refrain from setting the file descriptor limit to the default maximum.
    # https://stackoverflow.com/a/16535804/5208808
    java_args += ["-XX:-MaxFDLimit", "-Xss2m"]
    kwargs["java_args_for_run_info"] = java_args
    return fb_native.java_binary(name = name, **kwargs)

def _toolchain_prebuilt_jar(name, **kwargs):
    kwargs = _add_labels(**kwargs)
    kwargs = _set_buck2_dex_toolchain(**kwargs)
    if kwargs.pop("should_generate_snapshot", True) == False:
        kwargs["_prebuilt_jar_toolchain"] = "toolchains//:prebuilt_jar_bootstrap_no_snapshot"
    else:
        kwargs["_prebuilt_jar_toolchain"] = "toolchains//:prebuilt_jar_bootstrap"
    return fb_native.prebuilt_jar(name = name, **kwargs)

# `sha256` and `size_bytes` let Buck2 take the file from the remote cache
# instead of its origin: a cache keyed by SHA-256 (the open source default)
# can't look a download up by its SHA-1, so with a SHA-1 alone every new
# daemon fetches it from its origin, even when everything built from it is a
# cache hit.
def _oss_remote_file_with_wrapper(name, ext, url, sha1, sha256 = None, size_bytes = None, **kwargs):
    remote_file_target_name = name + "_" + ext
    if ext == "jar":
        _toolchain_prebuilt_jar(name = name, binary_jar = ":" + remote_file_target_name, **kwargs)
    elif ext == "aar":
        fb_native.android_prebuilt_aar(name = name, aar = ":" + remote_file_target_name, **kwargs)
    elif ext == "exe":
        fb_native.alias(name = name, actual = ":" + remote_file_target_name, **kwargs)

    fb_native.remote_file(
        name = remote_file_target_name,
        out = name + "." + ext,
        sha1 = sha1,
        sha256 = sha256,
        size_bytes = size_bytes,
        url = url,
        type = "executable" if ext == "exe" else "data",
    )

def _buck_remote_file_with_wrapper(
    name,
    ext,
    url,
    sha1,
    # @oss-disable[end= ]: internal_alias,
    sha256 = None,
    size_bytes = None,
    **kwargs,
):
    if not is_full_meta_repo():
        return _oss_remote_file_with_wrapper(name, ext, url, sha1, sha256 = sha256, size_bytes = size_bytes, **kwargs)
    # @oss-disable[end= ]: else:
        # @oss-disable: # deps are managed by Artificer internally - only relevant for OSS builds.
        # @oss-disable[end= ]: kwargs.pop("deps", None)
        # @oss-disable[end= ]: return native.alias(name = name, actual = internal_alias, **kwargs)
        fail() # @oss-enable

def third_party_jar(
    name,
    url,
    sha1,
    # @oss-disable[end= ]: internal_alias,
    sha256 = None,
    size_bytes = None,
    **kwargs,
):
    return _buck_remote_file_with_wrapper(
        name,
        "jar",
        url,
        sha1,
        # @oss-disable[end= ]: internal_alias,
        sha256 = sha256,
        size_bytes = size_bytes,
        **kwargs,
    )

def third_party_aar(
    name,
    url,
    sha1,
    # @oss-disable[end= ]: internal_alias,
    sha256 = None,
    size_bytes = None,
    **kwargs,
):
    return _buck_remote_file_with_wrapper(
        name,
        "aar",
        url,
        sha1,
        # @oss-disable[end= ]: internal_alias,
        sha256 = sha256,
        size_bytes = size_bytes,
        **kwargs,
    )

def third_party_exe(
    name,
    url,
    sha1,
    # @oss-disable[end= ]: internal_alias,
    sha256 = None,
    size_bytes = None,
    **kwargs,
):
    return _buck_remote_file_with_wrapper(
        name,
        "exe",
        url,
        sha1,
        # @oss-disable[end= ]: internal_alias,
        sha256 = sha256,
        size_bytes = size_bytes,
        **kwargs,
    )

def buck_prebuilt_jar(name, **kwargs):
    return _toolchain_prebuilt_jar(name = name, **kwargs)

def _shallow_dict_copy_without_key(table, key_to_omit):
    """Returns a shallow copy of dict with key_to_omit omitted."""
    return {key: table[key] for key in table if key != key_to_omit}

def buck_kotlin_test(**kwargs):
    extra_labels = [_RUN_AS_BUNDLE_LABEL, _FDB_DEBUG_LABEL]

    kwargs = _add_labels(**kwargs)
    kwargs = add_os_labels(**kwargs)
    kwargs["labels"] += extra_labels

    kwargs = _add_kotlin_deps(**kwargs)
    kwargs = _maybe_add_java_version(**kwargs)

    fb_native.kotlin_test(**kwargs)

def buck_java_test(name, vm_args = None, run_test_separately = False, **kwargs):
    """java_test wrapper that provides sensible defaults for buck tests.

    Args:
      name: name
      vm_args: vm_args
      run_test_separately: run_test_separately
      **kwargs: kwargs
    """

    extra_labels = [_RUN_AS_BUNDLE_LABEL, _FDB_DEBUG_LABEL]

    # Windows command line is short and running a bundle with many tests can cause problems
    # We fix this by running bundles of max 100 tests
    if native.host_info().os.is_windows:
        extra_labels.append("tpx:experimental-shard-size-for-bundle=100")

    if run_test_separately:
        extra_labels.append("serialize")

    if "deps" in kwargs:
        deps = kwargs["deps"]
        kwargs = _shallow_dict_copy_without_key(kwargs, "deps")
    else:
        deps = []

    if "env" in kwargs:
        env = kwargs["env"]
        kwargs = _shallow_dict_copy_without_key(kwargs, "env")
    else:
        env = {}

    kwargs = _maybe_add_java_version(**kwargs)
    kwargs = _add_labels(**kwargs)
    kwargs = add_os_labels(**kwargs)
    kwargs["labels"] += extra_labels

    fb_native.java_test(
        name = name,
        deps = deps
        + [
            # When actually running Buck, the launcher script loads the bootstrapper,
            # and the bootstrapper loads the rest of Buck. For unit tests, which don't
            # run Buck, we have to add a direct dependency on the bootstrapper in case
            # they exercise code that uses it.
            "prelude//toolchains/android/src/com/facebook/buck/cli/bootstrapper:bootstrapper_lib",
        ],
        vm_args = [
            # Don't use the system-installed JNA; extract it from the local jar.
            "-Djna.nosys=true",
            # Add -Dsun.zip.disableMemoryMapping=true to work around a JDK issue
            # related to modifying JAR/ZIP files that have been loaded into memory:
            #
            # http://bugs.sun.com/view_bug.do?bug_id=7129299
            #
            # This has been observed to cause a problem in integration tests such as
            # CachedTestIntegrationTest where `buck build //:test` is run repeatedly
            # such that a corresponding `test.jar` file is overwritten several times.
            # The CompiledClassFileFinder in JavaTestRule creates a java.util.zip.ZipFile
            # to enumerate the zip entries in order to find the set of .class files
            # in `test.jar`. This interleaving of reads and writes appears to match
            # the conditions to trigger the issue reported on bugs.sun.com.
            #
            # Currently, we do not set this flag in bin/buck_common, as Buck does not
            # normally modify the contents of buck-out after they are loaded into
            # memory. However, we may need to use this flag when running buckd where
            # references to zip files may be long-lived.
            #
            # Finally, note that when you specify this flag,
            # `System.getProperty("sun.zip.disableMemoryMapping")` will return `null`
            # even though you have specified the flag correctly. Apparently sun.misc.VM
            # (http://www.docjar.com/html/api/sun/misc/VM.java.html) saves the property
            # internally, but removes it from the set of system properties that are
            # publicly accessible.
            "-Dsun.zip.disableMemoryMapping=true",
        ]
        + (vm_args or []),
        env = env,
        run_test_separately = run_test_separately,
        **kwargs,
    )

def standard_java_test(name, run_test_separately = False, vm_args = None, labels = None, with_test_data = False, **kwargs):
    test_srcs = native.glob(["*Test.java"])

    if len(test_srcs) > 0:
        buck_java_test(
            name = name,
            srcs = test_srcs,
            resources = native.glob(["testdata/**"]) if with_test_data else [],
            vm_args = vm_args,
            run_test_separately = run_test_separately,
            labels = (labels or []) + ["buck2_run_from_cell_root"],
            **kwargs,
        )

def buck_prebuilt_artifact(
    # @oss-disable[end= ]: cas_digest,
    oss_url = None,
    oss_sha1 = None,
    oss_sha256 = None,
    oss_size_bytes = None,
    **kwargs,
):
    if (not is_full_meta_repo()) and oss_url:
        return fb_native.remote_file(sha1 = oss_sha1, sha256 = oss_sha256, size_bytes = oss_size_bytes, url = oss_url, **kwargs)
    # @oss-disable[end= ]: else:
        # @oss-disable[end= ]: return android_build_tools_cas_artifact(digest = cas_digest, **kwargs)
        fail() # @oss-enable
