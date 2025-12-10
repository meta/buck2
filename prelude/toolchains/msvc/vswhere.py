#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Translated from the Rust `cc` crate's windows_registry.rs.
# https://github.com/rust-lang/cc-rs/blob/1.0.79/src/windows_registry.rs

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import winreg
from pathlib import Path
from typing import IO, NamedTuple

VC_EXE_NAMES = ["cl.exe", "cvtres.exe", "lib.exe", "ml64.exe", "link.exe"]
UCRT_EXE_NAMES = ["rc.exe"]


class OutputJsonFiles(NamedTuple):
    # We write a Tool instance as JSON into each of these files.
    cl: IO[str]
    cvtres: IO[str]
    lib: IO[str]
    ml64: IO[str]
    link: IO[str]
    rc: IO[str]
    csc: IO[str]


class Tool(NamedTuple):
    exe: Path
    LIB: list[Path] = []
    PATH: list[Path] = []
    INCLUDE: list[Path] = []


def find_in_path(executable, is_optional=False):
    which = shutil.which(executable)
    if which is None:
        if is_optional:
            return None
        else:
            print(f"{executable} not found in $PATH", file=sys.stderr)
            sys.exit(1)
    return Tool(which)


def run_vswhere(required_component):
    program_files = os.environ.get("ProgramFiles(x86)")
    if program_files is None:
        program_files = os.environ.get("ProgramFiles")
    if program_files is None:
        print(
            "expected a %ProgramFiles(x86)% or %ProgramFiles% environment variable",
            file=sys.stderr,
        )
        sys.exit(1)

    vswhere_exe = (
        Path(program_files) / "Microsoft Visual Studio" / "Installer" / "vswhere.exe"
    )
    vswhere_json = subprocess.check_output(
        [
            vswhere_exe,
            "-products",
            "*",
            "-requires",
            required_component,
            "-format",
            "json",
            "-utf8",
            "-nologo",
        ],
        encoding="utf-8",
    )

    vswhere_json = json.loads(vswhere_json)

    # Sort by MSVC version, newest to oldest.
    # Version is a sequence of 16-bit integers.
    # Example: "17.6.33829.357"
    vswhere_json.sort(
        key=lambda vs: [int(n) for n in vs["installationVersion"].split(".")],
        reverse=True,
    )

    return vswhere_json


def find_msvc_with_vswhere_exe():
    vswhere_json = run_vswhere("Microsoft.VisualStudio.Component.VC.Tools.x86.x64")

    for vs_instance in list(vswhere_json):
        installation_path = Path(vs_instance["installationPath"])

        # Tools version is different from the one above: "14.36.32532"
        version_file = (
            installation_path
            / "VC"
            / "Auxiliary"
            / "Build"
            / "Microsoft.VCToolsVersion.default.txt"
        )
        vc_tools_version = version_file.read_text(encoding="utf-8").strip()

        tools_path = installation_path / "VC" / "Tools" / "MSVC" / vc_tools_version
        bin_path = tools_path / "bin" / "HostX64" / "x64"
        lib_path = tools_path / "lib" / "x64"
        include_path = tools_path / "include"

        vc_exe_paths = [bin_path / exe for exe in VC_EXE_NAMES]

        if not all(exe.exists() for exe in vc_exe_paths):
            continue

        PATH = [bin_path]
        LIB = [lib_path]
        INCLUDE = [include_path]

        ucrt, ucrt_version = get_ucrt_dir()
        if ucrt and ucrt_version:
            ucrt_bin_path = ucrt / "bin" / ucrt_version / "x64"
            PATH.append(ucrt_bin_path)
            LIB.append(ucrt / "lib" / ucrt_version / "ucrt" / "x64")
            INCLUDE.append(ucrt / "include" / ucrt_version / "ucrt")

            ucrt_exe_paths = [ucrt_bin_path / exe for exe in UCRT_EXE_NAMES]
            ucrt_exe_paths = [exe if exe.exists() else None for exe in ucrt_exe_paths]
        else:
            ucrt_exe_paths = [None for exe in UCRT_EXE_NAMES]

        sdk, sdk_version = get_sdk10_dir()
        if sdk and sdk_version:
            PATH.append(sdk / "bin" / "x64")
            LIB.append(sdk / "lib" / sdk_version / "um" / "x64")
            INCLUDE.append(sdk / "include" / sdk_version / "um")
            INCLUDE.append(sdk / "include" / sdk_version / "cppwinrt")
            INCLUDE.append(sdk / "include" / sdk_version / "winrt")
            INCLUDE.append(sdk / "include" / sdk_version / "shared")

        return [
            Tool(exe=exe, LIB=LIB, PATH=PATH, INCLUDE=INCLUDE)
            for exe in vc_exe_paths + ucrt_exe_paths
        ]

    print(
        "vswhere.exe did not find a suitable MSVC toolchain containing "
        + ", ".join(VC_EXE_NAMES),
        file=sys.stderr,
    )
    sys.exit(1)


def find_roslyn_with_vswhere_exe():
    vswhere_json = run_vswhere("Microsoft.VisualStudio.Component.Roslyn.Compiler")

    for vs_instance in list(vswhere_json):
        installation_path = Path(vs_instance["installationPath"])

        roslyn_path = (
            installation_path
            / "MSBuild"
            / "Current"
            / "Bin"
            / "Roslyn"
            / "csc.exe"
        )

        if not roslyn_path.exists():
            continue

        return Tool(exe=roslyn_path, LIB=[], PATH=[], INCLUDE=[])

    print(
        "vswhere.exe did not find a suitable Roslyn toolchain containing csc.exe",
        file=sys.stderr,
    )
    sys.exit(1)

# To find the Universal CRT we look in a specific registry key for where all the
# Universal CRTs are located and then sort them asciibetically to find the
# newest version. While this sort of sorting isn't ideal, it is what vcvars does
# so that's good enough for us.
#
# Returns a pair of (root, version) for the ucrt dir if found.
def get_ucrt_dir():
    registry = winreg.ConnectRegistry(None, winreg.HKEY_LOCAL_MACHINE)
    key_name = "SOFTWARE\\Microsoft\\Windows Kits\\Installed Roots"
    registry_key = winreg.OpenKey(registry, key_name)
    kits_root = Path(winreg.QueryValueEx(registry_key, "KitsRoot10")[0])

    available_versions = [
        entry.name
        for entry in kits_root.joinpath("lib").iterdir()
        if entry.name.startswith("10.") and entry.joinpath("ucrt").is_dir()
    ]

    max_version = max(available_versions) if available_versions else None
    return kits_root, max_version


# Vcvars finds the correct version of the Windows 10 SDK by looking for the
# include `um\Windows.h` because sometimes a given version will only have UCRT
# bits without the rest of the SDK. Since we only care about libraries and not
# includes, we instead look for `um\x64\kernel32.lib`. Since the 32-bit and
# 64-bit libraries are always installed together we only need to bother checking
# x64, making this code a tiny bit simpler. Like we do for the Universal CRT, we
# sort the possibilities asciibetically to find the newest one as that is what
# vcvars does. Before doing that, we check the "WindowsSdkDir" and
# "WindowsSDKVersion" environment variables set by vcvars to use the environment
# sdk version if one is already configured.
#
# Returns a pair of (root, version).
def get_sdk10_dir():
    windows_sdk_dir = os.environ.get("WindowsSdkDir")
    windows_sdk_version = os.environ.get("WindowsSDKVersion")
    if windows_sdk_dir is not None and windows_sdk_version is not None:
        return Path(windows_sdk_dir), windows_sdk_version.removesuffix("\\")

    registry = winreg.ConnectRegistry(None, winreg.HKEY_LOCAL_MACHINE)
    key_name = "SOFTWARE\\Microsoft\\Microsoft SDKs\\Windows\\v10.0"
    registry_key = winreg.OpenKey(
        registry, key_name, access=winreg.KEY_READ | winreg.KEY_WOW64_32KEY
    )
    installation_folder = Path(
        winreg.QueryValueEx(registry_key, "InstallationFolder")[0]
    )

    available_versions = [
        entry.name
        for entry in installation_folder.joinpath("lib").iterdir()
        if entry.joinpath("um", "x64", "kernel32.lib").is_file()
    ]

    max_version = max(available_versions) if available_versions else None
    return installation_folder, max_version


def write_tool_json(out, tool):
    j = json.dumps(
        tool._asdict(),
        indent=4,
        default=lambda path: str(path),
    )
    out.write(j)


# for use with the ewdk to grab the environment strings
def get_ewdk_env(ewdkdir: Path):
    """
    Inspiration taken from the following:
    http://pythonwise.blogspot.fr/2010/04/sourcing-shell-script.html (Miki Tebeka)
    http://stackoverflow.com/questions/3503719/#comment28061110_3505826 (ahal)
    """

    # We need to write the script that will make the important variables available
    with tempfile.NamedTemporaryFile(
        prefix="VcVarsExtract", suffix=".bat", mode="w", delete=False
    ) as tmp:
        print("@echo off", file=tmp)
        print("call %* > NUL", file=tmp)
        print("set", file=tmp)

    env_script = ewdkdir / "BuildEnv" / "SetupBuildEnv.cmd"
    cmd = [tmp.name, env_script, "amd64"]
    output = subprocess.check_output(cmd).decode("utf-8")

    env = {}
    for line in output.split("\r\n"):
        if line and "=" in line:
            first, second = line.split("=", 1)
            env[first] = second

    return env


def find_with_ewdk(ewdkdir: Path):
    env = get_ewdk_env(ewdkdir)

    installation_path = Path(env["VSINSTALLDIR"])
    vc_tools_version = env["VCToolsVersion"]
    tools_path = installation_path / "VC" / "Tools" / "MSVC" / vc_tools_version
    bin_path = tools_path / "bin" / "HostX64" / "x64"
    lib_path = tools_path / "lib" / "x64"
    include_path = tools_path / "include"

    PATH = [bin_path]
    LIB = [lib_path]
    INCLUDE = [include_path]

    ucrt = Path(env["UCRTContentRoot"])
    ucrt_version = env.get("Version_Number")

    vc_exe_paths = [bin_path / exe for exe in VC_EXE_NAMES]

    if ucrt_version:
        ucrt_bin_path = ucrt / "bin" / ucrt_version / "x64"
        PATH.append(ucrt_bin_path)
        LIB.append(ucrt / "lib" / ucrt_version / "ucrt" / "x64")
        INCLUDE.append(ucrt / "include" / ucrt_version / "ucrt")

        ucrt_exe_paths = [ucrt_bin_path / exe for exe in UCRT_EXE_NAMES]
        ucrt_exe_paths = [exe if exe.exists() else None for exe in ucrt_exe_paths]
    else:
        ucrt_exe_paths = [None for exe in UCRT_EXE_NAMES]

    sdk = Path(env["WindowsSdkDir"])
    sdk_version = ucrt_version
    if sdk_version:
        PATH.append(sdk / "bin" / "x64")
        LIB.append(sdk / "lib" / sdk_version / "um" / "x64")
        INCLUDE.append(sdk / "include" / sdk_version / "um")
        INCLUDE.append(sdk / "include" / sdk_version / "cppwinrt")
        INCLUDE.append(sdk / "include" / sdk_version / "winrt")
        INCLUDE.append(sdk / "include" / sdk_version / "shared")

    return [
        Tool(exe=bin_path / exe, LIB=LIB, PATH=PATH, INCLUDE=INCLUDE)
        for exe in vc_exe_paths + ucrt_exe_paths
    ]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cl", type=argparse.FileType("w"))
    parser.add_argument("--cvtres", type=argparse.FileType("w"))
    parser.add_argument("--lib", type=argparse.FileType("w"))
    parser.add_argument("--ml64", type=argparse.FileType("w"))
    parser.add_argument("--link", type=argparse.FileType("w"))
    parser.add_argument("--rc", type=argparse.FileType("w"))
    parser.add_argument("--csc", type=argparse.FileType("w"))
    output = OutputJsonFiles(**vars(parser.parse_args()))

    if output.cl or output.cvtres or output.lib or output.ml64 or output.link or output.rc:
        # If vcvars has been run, it puts these tools onto $PATH.
        if "VCINSTALLDIR" in os.environ:
            cl_exe, cvtres_exe, lib_exe, ml64_exe, link_exe = (
                find_in_path(exe) for exe in VC_EXE_NAMES
            )
            rc_exe = find_in_path("rc.exe", is_optional=True)
        elif "EWDKDIR" in os.environ:
            cl_exe, cvtres_exe, lib_exe, ml64_exe, link_exe, rc_exe = find_with_ewdk(
                Path(os.environ["EWDKDIR"])
            )
        else:
            cl_exe, cvtres_exe, lib_exe, ml64_exe, link_exe, rc_exe = (
                find_msvc_with_vswhere_exe()
            )

        write_tool_json(output.cl, cl_exe)
        write_tool_json(output.cvtres, cvtres_exe)
        write_tool_json(output.lib, lib_exe)
        write_tool_json(output.ml64, ml64_exe)
        write_tool_json(output.link, link_exe)
        write_tool_json(output.rc, rc_exe)

    if output.csc:
        csc_exe = find_roslyn_with_vswhere_exe()

        write_tool_json(output.csc, csc_exe)

if __name__ == "__main__":
    main()
