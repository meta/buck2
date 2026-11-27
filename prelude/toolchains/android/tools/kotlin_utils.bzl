# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""The Kotlin compiler version, and compiler options gated on it, for Android toolchain targets."""

load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")

# The Kotlin version of the compiler jars in `prelude//toolchains/android/third-party` when
# `kotlin.kotlin_version` is unset. It needs a row in `_KOTLIN_JAR_SHA1`.
DEFAULT_KOTLIN_VERSION = "2.3.21"

# The sha1 of each Kotlin jar in `prelude//toolchains/android/third-party` that follows
# `kotlin_version()`, for every Kotlin version those jars can have.
_KOTLIN_JAR_SHA1 = {
    "2.3.21": {
        "jvm-abi-gen": "9d6dad64b4468307ac8ddbe4d4712a208c7b1738",
        "kotlin-annotation-processing-embeddable": "4d68676c8b1a36043d3188f0de16f6842709bc1f",
        "kotlin-build-tools-api": "9b0a49a4d4c570c63a3a2553816bf0539536ab69",
        "kotlin-build-tools-impl": "c6c11fd63ff15677c11f14dbdbad624f8d8dbf04",
        "kotlin-compiler-embeddable": "c4b47f8e33cdbeedd8c1415ab9f9ade870388162",
        "kotlin-compose-compiler-plugin-embeddable": "96eb34ab9364f1c4291f35b38b1b98d0fc70170c",
        "kotlin-daemon-client": "8bd63e265ea5e6472f96f7e9e2431d02b4e69f3e",
        "kotlin-metadata-jvm": "44a462b3cacd654503a791e4a04ec84f7a1f1c78",
        "kotlin-reflect": "6acc55c52691ce3fd41b20a0472f3513106521a5",
        "kotlin-script-runtime": "8dafd7f0eb5efef35a446683a2774fef61c68aeb",
        "kotlin-stdlib": "a2ee2c4e220fee522f0451b723bab2a43f3481a7",
        "kotlin-tooling-core": "7a050c09910ee587944bc2a7413e2d97d751f647",
    },
    "2.4.21": {
        "jvm-abi-gen": "27d6c080368728b7e55dfa9ecdadb5b41be8d86e",
        "kotlin-annotation-processing-embeddable": "41f6adbb24a91a5aa4690f2453668040e5413f85",
        "kotlin-build-tools-api": "f5fd68c20ba38519b91ab2411b004a1730453d6f",
        "kotlin-build-tools-impl": "898086a64b68b1e9131900909e0fc056a782f37f",
        "kotlin-compiler-embeddable": "3cd9c37c505fb9879e03ebeaf91d9c9500ab6eca",
        "kotlin-compose-compiler-plugin-embeddable": "dd3cf05b1c6bfaf6c82869e1a0e5cf00df6b26b1",
        "kotlin-daemon-client": "6fc0b01cd07b02b22ec967a50f6aa7677ea9c0c9",
        "kotlin-metadata-jvm": "f5c8df1757bdaa216ec464bd7a025af4c4ca5bed",
        "kotlin-reflect": "0123e9b5b31f85b43025149fb1a34bb569288157",
        "kotlin-script-runtime": "abc7ff9f7e229cb6bdfadad81e93c871e6cdde41",
        "kotlin-stdlib": "c343e135bd1e7dd94b7ed76b4159068d888d09db",
        "kotlin-tooling-core": "a92e365882e147d1054b6245d2aa822cbaa77ad6",
    },
}

def kotlin_version() -> str:
    """The Kotlin compiler version: `kotlin.kotlin_version`, else `DEFAULT_KOTLIN_VERSION`."""
    return read_root_config("kotlin", "kotlin_version", DEFAULT_KOTLIN_VERSION)

def kotlin_jar_url(artifact: str) -> str:
    """The Maven URL of `org.jetbrains.kotlin:<artifact>` at `kotlin_version()`."""
    return "mvn:org.jetbrains.kotlin:{}:jar:{}".format(artifact, kotlin_version())

def kotlin_jar_sha1(artifact: str) -> str:
    """The sha1 of the jar at `kotlin_jar_url(artifact)`."""
    sha1 = _KOTLIN_JAR_SHA1.get(kotlin_version(), {}).get(artifact)
    if sha1 != None:
        return sha1
    if is_full_meta_repo():
        # Internal builds alias these jars and do not download them.
        return ""
    fail(
        "No sha1 for {} {}: Kotlin {} needs a row in `_KOTLIN_JAR_SHA1` in {}".format(
            artifact,
            kotlin_version(),
            kotlin_version(),
            "prelude/toolchains/android/tools/kotlin_utils.bzl",
        )
    )

def _leading_int(part: str) -> int:
    digits = ""
    for char in part.elems():
        if not char.isdigit():
            break
        digits += char
    return int(digits) if digits else 0

def kotlin_version_at_least(major: int, minor: int, version: str | None = None) -> bool:
    """Compare the actual compiler's major/minor, ignoring prerelease suffixes."""
    if version == None:
        version = kotlin_version()
    parts = version.split(".")
    if len(parts) < 2:
        return False
    return (_leading_int(parts[0]), _leading_int(parts[1])) >= (major, minor)

def kotlin_implementation_detail_opt_in_args(version: str | None = None) -> list[str]:
    """KtFile implementations require the PSI marker starting with Kotlin 2.5."""
    if kotlin_version_at_least(2, 5, version):
        return ["-opt-in=org.jetbrains.kotlin.psi.KtImplementationDetail"]
    return []
