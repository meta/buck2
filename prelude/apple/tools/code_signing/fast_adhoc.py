# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import logging
import os
import plistlib
import subprocess
import sys
from pathlib import Path
from typing import List, Optional, Tuple, Union
from xml.parsers.expat import ExpatError

# @oss-disable[end= ]: from ..meta_only.codesign_rust.check_adhoc_signature import (
    # @oss-disable[end= ]: read_signature_info,
# @oss-disable[end= ]: )
from .apple_platform import ApplePlatform

_LOGGER: logging.Logger = logging.getLogger(__name__)


def _find_executable_for_signed_path(
    path: Path, platform: ApplePlatform
) -> Optional[Path]:
    extension = path.suffix
    if extension not in [".app", ".appex", ".framework"]:
        return path

    contents_subdir = "Contents/MacOS" if platform.is_desktop() else ""
    contents_dir = path / contents_subdir

    # Bundles without a readable CFBundleExecutable (e.g. versioned macOS
    # frameworks, synthetic test bundles) cannot be proven pre-signed, so
    # callers fall back to the normal signing path.
    info_plist_path = path / (
        "Contents/Info.plist" if platform.is_desktop() else "Info.plist"
    )
    try:
        with open(info_plist_path, "rb") as file:
            plist = plistlib.load(file)
    except (OSError, plistlib.InvalidFileException, ExpatError):
        return None
    executable_name = plist.get("CFBundleExecutable")
    if not isinstance(executable_name, str):
        return None

    return contents_dir / executable_name


def _logged_subprocess_run(
    name: str, why: str, args: List[Union[str, Path]]
) -> subprocess.CompletedProcess[str]:
    _LOGGER.info(f"  Calling {name} to {why}: `{args}`")
    result = subprocess.run(
        args,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        encoding="utf-8",
    )

    _LOGGER.info(f"  {name} exit code: {result.returncode}")
    if result.stdout:
        _LOGGER.info(f"  {name} stdout:")
        _LOGGER.info(
            "\n" + "\n".join([f"    {line}" for line in result.stdout.split("\n")])
        )
    if result.stderr:
        _LOGGER.info(f"  {name} stderr:")
        _LOGGER.info(
            "\n" + "\n".join([f"    {line}" for line in result.stderr.split("\n")])
        )

    return result


def is_fast_adhoc_codesign_allowed(probe_enabled: bool) -> bool:
    if sys.platform == "darwin":
        # Xcode's active-developer-dir symlink is /var/db/xcode_select_link on
        # older macOS and /var/select/developer_dir on Sonoma+/Xcode 15+.
        if not os.path.exists("/var/db/xcode_select_link") and not os.path.exists(
            "/var/select/developer_dir"
        ):
            _LOGGER.info(
                "Developer tools do not exist, cannot use `otool`, fast adhoc signing not allowed"
            )
            return False
        return True
    # @oss-disable[end= ]: if probe_enabled:
        # @oss-disable[end= ]: return True
    _LOGGER.info(f"Running on {sys.platform}, fast adhoc signing not allowed")
    return False


def _read_signature_info_macos(
    path: Path,
    platform: ApplePlatform,
    check_entitlements: bool,
) -> Optional[Tuple[bool, bool]]:
    """Reads adhoc/entitlements info via `/usr/bin/codesign` and `/usr/bin/otool`.

    The entitlements probe runs only when `check_entitlements=True` and the path
    is adhoc-signed; a non-adhoc path is never skipped, so its entitlements are
    never inspected and we return early to avoid a wasted `otool` subprocess.
    """
    codesign_args: List[Union[str, Path]] = ["codesign", "-d", "-v", path]
    codesign_result = _logged_subprocess_run(
        "codesign", "check pre-existing signature", codesign_args
    )

    # Anything that's _already_ adhoc signed can be skipped.
    # On ARM64 systems, the linker will already codesign using adhoc,
    # so performing the signing twice is unnecessary.
    is_adhoc = "Signature=adhoc" in codesign_result.stderr

    if not is_adhoc:
        return (False, False)

    has_entitlements_section = False
    if check_entitlements:
        # Adhoc entitlements do not require postprocessing, so we just need to check existence
        binary_path = _find_executable_for_signed_path(path, platform)
        if binary_path is None:
            _LOGGER.info(
                f"  Could not find an executable for `{path}`, not skipping adhoc signing"
            )
            return None
        otool_arg: List[Union[str, Path]] = [
            "/usr/bin/otool",
            "-s",
            "__TEXT",
            "__entitlements",
            binary_path,
        ]
        otool_result = _logged_subprocess_run(
            "otool", "check entitlements presence in binary", otool_arg
        )
        has_entitlements_section = (
            "Contents of (__TEXT,__entitlements) section" in otool_result.stdout
        )

    return (is_adhoc, has_entitlements_section)


def _read_signature_info(
    path: Path,
    platform: ApplePlatform,
    check_entitlements: bool,
) -> Optional[Tuple[bool, bool]]:
    """Platform-dispatching read of signature info.

    Returns None on unsupported platforms; should_skip_adhoc_signing_path
    treats None as "could not verify, do not skip".
    """
    if sys.platform == "darwin":
        return _read_signature_info_macos(path, platform, check_entitlements)
    # @oss-disable[end= ]: elif sys.platform == "linux":
        # @oss-disable[end= ]: binary = _find_executable_for_signed_path(path, platform)
        # @oss-disable[end= ]: if binary is None:
            # @oss-disable[end= ]: _LOGGER.info(
                # @oss-disable[end= ]: f"  Could not find an executable for `{path}`, not skipping adhoc signing"
            # @oss-disable[end= ]: )
            # @oss-disable[end= ]: return None
        # @oss-disable[end= ]: return read_signature_info(binary, check_entitlements)
    return None


def should_skip_adhoc_signing_path(
    path: Path,
    identity_fingerprint: str,
    entitlements_path: Optional[Path],
    platform: ApplePlatform,
) -> bool:
    logging.getLogger(__name__).info(
        f"Checking if should skip adhoc signing path `{path}` with identity `{identity_fingerprint}` and entitlements `{entitlements_path}` for platform `{platform}`"
    )

    if identity_fingerprint != "-":
        # Non-adhoc environments _always_ code sign
        _LOGGER.info("  Requested non-adhoc signing, not adhoc skipping signing")
        return False

    if "libclang_rt" in str(path):
        # Sanitizer runtime dylibs require re-signing, even though they're already pre-signed.
        # Otherwise, `codesign` fails to sign the top-level bundle (as the adhoc pre-signed
        # sanitizer dylibs have been signed within a different context).
        return False

    info = _read_signature_info(
        path, platform, check_entitlements=entitlements_path is not None
    )

    if info is None:
        _LOGGER.info("  Could not read signature info, not skipping adhoc signing")
        return False

    is_adhoc, has_entitlements_section = info
    if not is_adhoc:
        _LOGGER.info("  Path is not adhoc signed, not skipping adhoc signing")
        return False

    # The entitlements file can be ignored under adhoc signing because:
    #
    # - Frameworks/dylibs do not need entitlements (they operate under the entitlements of their loading binary)
    # - Apps (+ app extensions) have binaries which embed the entitlements via __entitlements section at link time
    #
    # Note that certain features require non-adhoc signing (e.g., app groups) while other features like keychain
    # and "Sign in with Apple" just need the entitlements present in the binary (which it will per the above).
    if entitlements_path and not has_entitlements_section:
        binary_path = _find_executable_for_signed_path(path, platform)
        _LOGGER.info(
            f"  Binary path `{binary_path}` does not contain entitlements, not skipping adhoc signing"
        )
        return False

    _LOGGER.info(f"  All checks passed for `{path}`, skipping adhoc signing")
    return True
