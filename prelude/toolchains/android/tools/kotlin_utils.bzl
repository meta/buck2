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
# `kotlin.kotlin_version` is unset. It needs a row in `_KOTLIN_JARS`.
DEFAULT_KOTLIN_VERSION = "2.4.21"

_KotlinJar = record(
    sha1 = str,
    sha256 = str,
    size_bytes = int,
)

# Each Kotlin jar in `prelude//toolchains/android/third-party` that follows `kotlin_version()`, for
# every Kotlin version those jars can have.
_KOTLIN_JARS = {
    "2.3.21": {
        "jvm-abi-gen": _KotlinJar(
            sha1 = "9d6dad64b4468307ac8ddbe4d4712a208c7b1738",
            sha256 = "88a5e104dde4ef2f3cd211b077bb93f0a5e2e54b61e53c373f146af061be4504",
            size_bytes = 969316,
        ),
        "kotlin-annotation-processing-embeddable": _KotlinJar(
            sha1 = "4d68676c8b1a36043d3188f0de16f6842709bc1f",
            sha256 = "7e8e36c0a349d67244c41e43adb4c482fa18ce0db042306515540f6c9ac2d476",
            size_bytes = 479140,
        ),
        "kotlin-build-tools-api": _KotlinJar(
            sha1 = "9b0a49a4d4c570c63a3a2553816bf0539536ab69",
            sha256 = "b2e5cc2666c100079ed343618528eb820dcfa5095095e30f2dd19c60dff7e462",
            size_bytes = 168792,
        ),
        "kotlin-build-tools-impl": _KotlinJar(
            sha1 = "c6c11fd63ff15677c11f14dbdbad624f8d8dbf04",
            sha256 = "2cd70396404a0e43c05aaf79a8b35ff3a8ff2e296aedf770f6e5923fbfacae3b",
            size_bytes = 1461208,
        ),
        "kotlin-compiler-embeddable": _KotlinJar(
            sha1 = "c4b47f8e33cdbeedd8c1415ab9f9ade870388162",
            sha256 = "d3e70fb011675e77ef00f1a68918d3cd91eed26058977d1e3ea2a37a293233af",
            size_bytes = 59720376,
        ),
        "kotlin-compose-compiler-plugin-embeddable": _KotlinJar(
            sha1 = "96eb34ab9364f1c4291f35b38b1b98d0fc70170c",
            sha256 = "6658edfd25de3dec89bdedc0308014a2ebd97731c6f749df0827c5be1ef95515",
            size_bytes = 941028,
        ),
        "kotlin-daemon-client": _KotlinJar(
            sha1 = "8bd63e265ea5e6472f96f7e9e2431d02b4e69f3e",
            sha256 = "fa767c9b40d069f164c617a6e6bd302643c7ba8c0792e8ed9e412c18f84b50be",
            size_bytes = 243178,
        ),
        "kotlin-metadata-jvm": _KotlinJar(
            sha1 = "44a462b3cacd654503a791e4a04ec84f7a1f1c78",
            sha256 = "00590f26a5a768777f04252c6cf93c016a3361a6bc93636df67375ad4d19557d",
            size_bytes = 902765,
        ),
        "kotlin-reflect": _KotlinJar(
            sha1 = "6acc55c52691ce3fd41b20a0472f3513106521a5",
            sha256 = "33e37d9dfa86c7a29ab34e975b5f4a1287b458878611051f3485652220c1a2cb",
            size_bytes = 3633922,
        ),
        "kotlin-script-runtime": _KotlinJar(
            sha1 = "8dafd7f0eb5efef35a446683a2774fef61c68aeb",
            sha256 = "ec61bf1229c837fa9f4255f34ca69816138b73158e7ae9b8d964cf92ca6dfd2e",
            size_bytes = 44972,
        ),
        "kotlin-stdlib": _KotlinJar(
            sha1 = "a2ee2c4e220fee522f0451b723bab2a43f3481a7",
            sha256 = "6f64eac736db9434dd6925b4a518b9d1d17177652320c37916cf9ba3ce7d7d7a",
            size_bytes = 1804721,
        ),
        "kotlin-tooling-core": _KotlinJar(
            sha1 = "7a050c09910ee587944bc2a7413e2d97d751f647",
            sha256 = "367142781299bc0f9120c1deec0e62934a1af9ea7f01aaa9c5a5354dc5d8d7d9",
            size_bytes = 99749,
        ),
    },
    "2.4.21": {
        "jvm-abi-gen": _KotlinJar(
            sha1 = "27d6c080368728b7e55dfa9ecdadb5b41be8d86e",
            sha256 = "e8ef7a82bc2df9c140d204fc0362f59b0e6df51c0406fb7360d18312da3be1cf",
            size_bytes = 977994,
        ),
        "kotlin-annotation-processing-embeddable": _KotlinJar(
            sha1 = "41f6adbb24a91a5aa4690f2453668040e5413f85",
            sha256 = "b798fabf9adf246e3091d0e8ae06a1b3d19feee5e399e3d13b61e27871f19692",
            size_bytes = 446216,
        ),
        "kotlin-build-tools-api": _KotlinJar(
            sha1 = "f5fd68c20ba38519b91ab2411b004a1730453d6f",
            sha256 = "c5cd6a49bd072b529ae10809111c16a7aa0f92147e95d288354017fd65799461",
            size_bytes = 468126,
        ),
        "kotlin-build-tools-impl": _KotlinJar(
            sha1 = "898086a64b68b1e9131900909e0fc056a782f37f",
            sha256 = "3152b3e1fbaa4d48f3681734a2c0efc438359a48c8a3c5cac00d874670de1377",
            size_bytes = 3456633,
        ),
        "kotlin-compiler-embeddable": _KotlinJar(
            sha1 = "3cd9c37c505fb9879e03ebeaf91d9c9500ab6eca",
            sha256 = "ef19419c765e7ac8404465fa026ca8fc4dbb8a822de0fc79aa53c8dce29f1d02",
            size_bytes = 58593291,
        ),
        "kotlin-compose-compiler-plugin-embeddable": _KotlinJar(
            sha1 = "dd3cf05b1c6bfaf6c82869e1a0e5cf00df6b26b1",
            sha256 = "634f5c17bed5854548208f2901071f2ead5993ffad25b34fea1668ee9e314b53",
            size_bytes = 865082,
        ),
        "kotlin-daemon-client": _KotlinJar(
            sha1 = "6fc0b01cd07b02b22ec967a50f6aa7677ea9c0c9",
            sha256 = "095903c4c4713bf91a562d226d7ba0464cf5424e76342d487deec494451ab29b",
            size_bytes = 265893,
        ),
        "kotlin-metadata-jvm": _KotlinJar(
            sha1 = "f5c8df1757bdaa216ec464bd7a025af4c4ca5bed",
            sha256 = "848e8db928d4265c0302ddb95544eb127b52b0be13bd724b31dbee6bc16d8223",
            size_bytes = 911360,
        ),
        "kotlin-reflect": _KotlinJar(
            sha1 = "0123e9b5b31f85b43025149fb1a34bb569288157",
            sha256 = "7c9e8bda3cafa85b65692065acd22578b2843292b4e4baa8361bdc9175746e88",
            size_bytes = 3808541,
        ),
        "kotlin-script-runtime": _KotlinJar(
            sha1 = "abc7ff9f7e229cb6bdfadad81e93c871e6cdde41",
            sha256 = "a658cf5902a1525a06c264a5e42b437e97a6679ddeef8ca641246f270cf6a013",
            size_bytes = 46533,
        ),
        "kotlin-stdlib": _KotlinJar(
            sha1 = "c343e135bd1e7dd94b7ed76b4159068d888d09db",
            sha256 = "bd8250210584cb659847dce9cda660f8e9906e5def7fbebcea93dc6dcb6a88a3",
            size_bytes = 1853315,
        ),
        "kotlin-tooling-core": _KotlinJar(
            sha1 = "a92e365882e147d1054b6245d2aa822cbaa77ad6",
            sha256 = "5fc5db083803232857408ed5c25bcc307c703b04bdef6cb2c34ebf4db611a622",
            size_bytes = 89152,
        ),
    },
}

def kotlin_version() -> str:
    """The Kotlin compiler version: `kotlin.kotlin_version`, else `DEFAULT_KOTLIN_VERSION`."""
    return read_root_config("kotlin", "kotlin_version", DEFAULT_KOTLIN_VERSION)

def kotlin_jar_url(artifact: str) -> str:
    """The Maven URL of `org.jetbrains.kotlin:<artifact>` at `kotlin_version()`."""
    return "mvn:org.jetbrains.kotlin:{}:jar:{}".format(artifact, kotlin_version())

def _kotlin_jar(artifact: str) -> _KotlinJar | None:
    jar = _KOTLIN_JARS.get(kotlin_version(), {}).get(artifact)
    if jar != None or is_full_meta_repo():
        # Internal builds alias these jars and do not download them.
        return jar
    fail(
        "No sha1 or sha256 for {} {}: Kotlin {} needs a row in `_KOTLIN_JARS` in {}".format(
            artifact,
            kotlin_version(),
            kotlin_version(),
            "prelude/toolchains/android/tools/kotlin_utils.bzl",
        )
    )

def kotlin_jar_sha1(artifact: str) -> str:
    """The sha1 of the jar at `kotlin_jar_url(artifact)`."""
    jar = _kotlin_jar(artifact)
    return jar.sha1 if jar else ""

def kotlin_jar_sha256(artifact: str) -> str | None:
    """The sha256 of the jar at `kotlin_jar_url(artifact)`."""
    jar = _kotlin_jar(artifact)
    return jar.sha256 if jar else None

def kotlin_jar_size_bytes(artifact: str) -> int | None:
    """The size of the jar at `kotlin_jar_url(artifact)`."""
    jar = _kotlin_jar(artifact)
    return jar.size_bytes if jar else None

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
