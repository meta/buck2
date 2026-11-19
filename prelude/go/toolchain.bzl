# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

GoVersion = record(minor = field(int), patch = field(int))

GoDistrInfo = provider(
    # @unsorted-dict-items
    fields = {
        "bin_go": provider_field(RunInfo),
        "go_root": provider_field(Artifact | None),
        "tool_asm": provider_field(RunInfo),
        "tool_compile": provider_field(RunInfo),
        "tool_cover": provider_field(RunInfo),
        "tool_cgo": provider_field(RunInfo),
        "tool_link": provider_field(RunInfo),
        "version": provider_field(GoVersion | None),
    },
)

GoToolchainInfo = provider(
    # @unsorted-dict-items
    fields = {
        "allow_cache_upload": provider_field(bool | None, default = None),
        "assembler": provider_field(RunInfo),
        "assembler_flags": provider_field(typing.Any, default = []),
        "cxx_compiler_flags": provider_field(typing.Any, default = []),
        "cgo": provider_field(RunInfo),
        "gen_embedcfg": provider_field(RunInfo),
        "go_wrapper": provider_field(RunInfo),
        "compiler": provider_field(RunInfo),
        "compiler_flags": provider_field(typing.Any, default = []),
        "pkg_analyzer": provider_field(RunInfo),
        "cover": provider_field(RunInfo),
        "env_go_arch": provider_field(str),
        "env_go_os": provider_field(str),
        "env_go_arm": provider_field(str | None, default = None),
        "env_go_root": provider_field(Artifact | None, default = None),
        "env_go_debug": provider_field(dict[str, str], default = {}),
        "env_go_experiment": provider_field(list[str], default = []),
        "external_linker_flags": provider_field(typing.Any, default = []),
        "go": provider_field(RunInfo),
        "linker": provider_field(RunInfo),
        "linker_flags": provider_field(typing.Any, default = []),
        "packer": provider_field(RunInfo),
        "build_tags": provider_field(list[str], default = []),
        "asan": provider_field(bool, default = False),
        "race": provider_field(bool, default = False),
        "fuzz": provider_field(bool, default = False),
        "version": provider_field(GoVersion | None),
    },
)

def get_toolchain_env_vars(toolchain: GoToolchainInfo) -> dict[str, str | cmd_args | Artifact]:
    env = {
        "GOARCH": toolchain.env_go_arch,
        "GOEXPERIMENT": ",".join(toolchain.env_go_experiment),
        "GOOS": toolchain.env_go_os,
    }

    if toolchain.env_go_arm != None:
        env["GOARM"] = toolchain.env_go_arm
    if toolchain.env_go_debug:
        godebug = ",".join(["{}={}".format(k, v) for k, v in toolchain.env_go_debug.items()])
        env["GODEBUG"] = godebug

    return env

def evaluate_cgo_enabled(cgo_enabled: bool, override_cgo_enabled: bool | None = None) -> bool:
    if override_cgo_enabled != None:
        return override_cgo_enabled

    return cgo_enabled

# Parse Go version from string, accepted format:
# - "1.20"
# - "1.22rc1" (for pre-release versions)
# - "1.22.1" (for patch versions)
# - "unknown" or "devel" (for development purposes)
def parse_go_version(version: str) -> GoVersion | None:
    if version == "unknown" or version == "devel":
        return None

    version_parts = version.split(".")

    if len(version_parts) < 2 or len(version_parts) > 3:
        fail("Invalid Go version: '{}'. Expected format '1.20', '1.22rc1', '1.22.1', 'devel' or 'unknown'.".format(version))

    if version_parts[0] != "1":
        fail("Invalid Go version: '{}'. Expected major version to be '1'.".format(version))

    if version_parts[1].isdigit():
        minor = int(version_parts[1])
    elif "rc" in version_parts[1]:
        rc_parts = version_parts[1].split("rc")
        if not (rc_parts[0].isdigit() and rc_parts[1].isdigit()):
            fail("Invalid Go version: '{}'. Expected minor version to be a number with an optional RC suffix like 'rc1'.".format(version))
        minor = int(rc_parts[0])
    else:
        fail("Invalid Go version: '{}'. Expected minor version to be a number with an optional RC suffix like 'rc1'.".format(version))

    patch = 0
    if len(version_parts) > 2:
        if not version_parts[2].isdigit():
            fail("Invalid Go version: '{}'. Expected patch version to be a number.".format(version))

        patch = int(version_parts[2])

    return GoVersion(minor = minor, patch = patch)
