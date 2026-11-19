# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import datetime
import os
import plistlib
import tempfile
import time
import unittest
from pathlib import Path

from . import codesign_bundle
from .apple_platform import ApplePlatform
from .identity import CodeSigningIdentity
from .info_plist_metadata import InfoPlistMetadata
from .list_codesign_identities import IListCodesignIdentities
from .provisioning_profile_metadata import ProvisioningProfileMetadata
from .provisioning_profile_selection import (
    CodeSignProvisioningError,
    select_best_provisioning_profile_core,
)


def _select_profile_expiring_in(hours: int):
    """Selects among one profile whose expiration date, a naive UTC time as plistlib reads
    it, lies `hours` from now."""
    now_utc = datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None)
    identity = CodeSigningIdentity("fingerprint", "name")
    profile = ProvisioningProfileMetadata(
        Path("/foo"),
        "00000000-0000-0000-0000-000000000000",
        now_utc + datetime.timedelta(hours=hours),
        {"iOS"},
        {identity.fingerprint},
        {"application-identifier": "ABCDEFGHIJ.com.company.application"},
    )
    selected, _ = select_best_provisioning_profile_core(
        info_plist_metadata=InfoPlistMetadata("com.company.application", None, False),
        code_signing_identities=[identity],
        provisioning_profiles=[profile],
        entitlements={},
        platform=ApplePlatform.ios_device,
        strict_search=False,
        provisioning_profile_filter=None,
    )
    return selected


class _LocalTimeZone:
    """Runs the body under a fixed POSIX `TZ`, which needs no time zone database."""

    def __init__(self, tz: str) -> None:
        self._tz = tz

    def __enter__(self) -> None:
        self._previous = os.environ.get("TZ")
        os.environ["TZ"] = self._tz
        time.tzset()

    def __exit__(self, *exc_info) -> None:
        if self._previous is None:
            del os.environ["TZ"]
        else:
            os.environ["TZ"] = self._previous
        time.tzset()


class TestProfileExpiryTimeZone(unittest.TestCase):
    def test_expiry_is_compared_with_utc(self) -> None:
        with _LocalTimeZone("PST8"):
            self.assertIsNone(_select_profile_expiring_in(-3))
        with _LocalTimeZone("IST-5:30"):
            self.assertIsNotNone(_select_profile_expiring_in(3))


class _NoIdentities(IListCodesignIdentities):
    def list_codesign_identities(self) -> list:
        return []

    def raw_command(self) -> str:
        return "none"


def _write_profile(directory: Path, expires_in_hours: int) -> None:
    """A `.mobileprovision` the fast parser reads: the plist with the fields selection needs,
    with its `ExpirationDate` written as plistlib writes a naive datetime (UTC, `Z`)."""
    now_utc = datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None)
    plist = plistlib.dumps(
        {
            "UUID": "00000000-0000-0000-0000-000000000000",
            "ExpirationDate": now_utc + datetime.timedelta(hours=expires_in_hours),
            "Platform": ["iOS"],
            "DeveloperCertificates": [b"certificate"],
            "Entitlements": {
                "application-identifier": "ABCDEFGHIJ.com.company.application"
            },
        }
    )
    (directory / "profile.mobileprovision").write_bytes(b"\x30\x82" + plist)


def _select_from_profiles_dir(expires_in_hours: int):
    """Selects through `codesign_bundle`, which reads the profile files from disk."""
    with tempfile.TemporaryDirectory() as temp_dir:
        _write_profile(Path(temp_dir), expires_in_hours)
        return codesign_bundle._select_best_provisioning_profile(
            info_plist_metadata=InfoPlistMetadata(
                "com.company.application", None, False
            ),
            provisioning_profiles_dirs=[Path(temp_dir)],
            entitlements_path=None,
            platform=ApplePlatform.ios_device,
            list_codesign_identities=_NoIdentities(),
            should_use_fast_provisioning_profile_parsing=True,
            strict_provisioning_profile_search=False,
            provisioning_profile_filter=None,
            no_check_certificates=True,
        )


class TestProfileExpiryFromFiles(unittest.TestCase):
    def test_expired_profile_file_is_rejected_west_of_utc(self) -> None:
        with _LocalTimeZone("PST8"):
            with self.assertRaises(CodeSignProvisioningError):
                _select_from_profiles_dir(-3)

    def test_valid_profile_file_is_selected_east_of_utc(self) -> None:
        with _LocalTimeZone("IST-5:30"):
            self.assertIsNotNone(_select_from_profiles_dir(3))
