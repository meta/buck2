# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

"""
Packages given input files into the correct format for an XCFramework, and generates
the required Info.plist.

Example Usage:
xcframework_maker.py --name FooKit --output_path /tmp/FooKit.xcframework \
        --framework_path ios-arm64 input/ios/FooKit.xcframework \
        --dsym_path ios-arm64 input/ios/dSYM \
        --framework_path ios-arm64_x86_64-simulator input/ios-simulator/FooKit.xcframework \
        --dsym_path ios-arm64_x86_64-simulator input/ios-simulator/dSYM 
"""

import argparse
import plistlib
import shutil
from pathlib import Path
from typing import Any, Optional

# functions that take architecture specifiers as 'item'.
# Examples:
# ios-arm64_x86_64-simulator
# -> supported platform: ios
# -> supported architectures [arm64, x86_64]
# -> supported platform variant: simulator
# watchos-arm64_arm64_32
# -> supported platform: watchos
# -> supported architectures: [arm64, arm64_32]
# -> supported platform variant: None


def _supported_architectures(item: str) -> list[str]:
    archs = []
    # order is important so that we can
    # consume 'arm64_32' first to prevent it
    # later matching arm64
    for arch in ["arm64_32", "arm64", "x86_64"]:
        if arch in item:
            archs.append(arch)
            item = item.replace(arch, "")
    return archs


def _supported_platform(item: str) -> str:
    return item.split("-")[0]


def _supported_platform_variant(item: str) -> Optional[str]:
    components = item.split("-")
    if len(components) > 2:
        return components[2]
    else:
        return None


def _make_plist_entry(
    item: str, binary_path: str, library_path: str, dsym_path: Optional[str]
) -> dict[str, Any]:
    entry = {
        "BinaryPath": binary_path,
        "LibraryIdentifier": item,
        "LibraryPath": library_path,
        "SupportedArchitectures": _supported_architectures(item),
        "SupportedPlatform": _supported_platform(item),
    }
    variant = _supported_platform_variant(item)
    if variant is not None:
        entry["SupportedPlatformVariant"] = variant

    if dsym_path is not None:
        entry["DebugSymbolsPath"] = dsym_path

    return entry


def _make_plist(
    items: list[str],
    binary_paths: list[str],
    library_path: str,
    dsym_paths: list[Optional[str]],
) -> bytes:
    d = {}
    d["AvailableLibraries"] = [
        _make_plist_entry(item, binary_path, library_path, dsym_path)
        for (item, binary_path, dsym_path) in zip(items, binary_paths, dsym_paths)
    ]
    d["CFBundlePackageType"] = "XFWK"
    d["XCFrameworkFormatVersion"] = "1.0"
    return plistlib.dumps(d)


def _find_binary_path(framework_fullpath: str, binary_name: str) -> str:
    fullpath = Path(framework_fullpath)
    versioned_binary_paths = sorted(fullpath.glob("Versions/Current/" + binary_name))
    if len(versioned_binary_paths) > 0:
        return versioned_binary_paths[-1].relative_to(fullpath.parents[0]).as_posix()
    return fullpath.name + "/" + binary_name


def main() -> None:
    parser = argparse.ArgumentParser(description="Tool to make an xcframework bundle.")
    parser.add_argument("--output-path")
    parser.add_argument("--name")
    parser.add_argument("--framework-path", action="append", nargs="+")
    parser.add_argument(
        "--dsym-path", action="append", nargs="+", default=[], required=False
    )
    args = parser.parse_args()

    out_path = Path(args.output_path)
    out_path.mkdir(parents=True, exist_ok=False)

    plist_path = out_path / "Info.plist"
    items = [fp_args[0] for fp_args in args.framework_path]
    binary_paths = []
    dsym_path_map = {}

    for framework_path in args.framework_path:
        # args are structured like this
        # --framework_path ios-arm64 buck-out/path/to/MyPkg.framework

        framework_arch = framework_path[0]
        framework_fullpath = framework_path[1]
        framework_basename = Path(framework_fullpath).name

        shutil.copytree(
            framework_fullpath,
            out_path / framework_arch / framework_basename,
            symlinks=True,
            dirs_exist_ok=False,
        )

        binary_paths.append(_find_binary_path(framework_fullpath, args.name))

    for dsym_path in args.dsym_path:
        dsym_arch = dsym_path[0]
        dsym_fullpath = dsym_path[1]
        shutil.copytree(
            dsym_fullpath,
            out_path / dsym_arch / "dSYMs" / (args.name + ".framework.dSYM"),
            symlinks=True,
            dirs_exist_ok=False,
        )
        dsym_path_map[dsym_arch] = "dSYMs"

    dsym_paths = [dsym_path_map.get(arch) for arch in items]

    library_path = args.name + ".framework"
    plist_path.write_bytes(_make_plist(items, binary_paths, library_path, dsym_paths))


if __name__ == "__main__":
    main()
