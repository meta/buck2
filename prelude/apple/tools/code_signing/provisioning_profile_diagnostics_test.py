# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import unittest
from datetime import datetime
from pathlib import Path
from unittest.mock import patch

from .apple_platform import ApplePlatform
from .codesign_bundle import _try_select_from_profiles_dir
from .identity import CodeSigningIdentity
from .info_plist_metadata import InfoPlistMetadata
from .provisioning_profile_metadata import ProvisioningProfileMetadata


def _make_profile(
    file_path: str = "/profiles/test.mobileprovision",
    bundle_id: str = "AAAAAAAAAA.com.company.app",
    fingerprint: str = "fingerprint",
    expired: bool = False,
) -> ProvisioningProfileMetadata:
    return ProvisioningProfileMetadata(
        Path(file_path),
        "00000000-0000-0000-0000-000000000000",
        datetime.min if expired else datetime.max,
        {"iOS"},
        {fingerprint},
        {"application-identifier": bundle_id},
    )


def _make_identity(
    fingerprint: str = "fingerprint",
) -> CodeSigningIdentity:
    return CodeSigningIdentity(fingerprint, "name")


class TestTrySelectFromProfilesDir(unittest.TestCase):
    """Tests for _try_select_from_profiles_dir helper."""

    def test_empty_dir_returns_none_tuple(self):
        with patch(
            "apple.tools.code_signing.codesign_bundle._read_profiles_from_dir",
            return_value=[],
        ):
            selected, profiles, mismatches = _try_select_from_profiles_dir(
                profiles_dir=Path("/empty"),
                info_plist_metadata=InfoPlistMetadata("com.company.app", None, False),
                identities=[_make_identity()],
                entitlements={},
                platform=ApplePlatform.ios_device,
                should_use_fast_provisioning_profile_parsing=False,
                strict_provisioning_profile_search=False,
                provisioning_profile_filter=None,
                no_check_certificates=False,
            )
        self.assertIsNone(selected)
        self.assertIsNone(profiles)
        self.assertIsNone(mismatches)

    def test_matching_profile_returns_selection(self):
        identity = _make_identity()
        profile = _make_profile()
        with patch(
            "apple.tools.code_signing.codesign_bundle._read_profiles_from_dir",
            return_value=[profile],
        ):
            selected, profiles, mismatches = _try_select_from_profiles_dir(
                profiles_dir=Path("/profiles"),
                info_plist_metadata=InfoPlistMetadata("com.company.app", None, False),
                identities=[identity],
                entitlements={},
                platform=ApplePlatform.ios_device,
                should_use_fast_provisioning_profile_parsing=False,
                strict_provisioning_profile_search=False,
                provisioning_profile_filter=None,
                no_check_certificates=False,
            )
        self.assertIsNotNone(selected)
        self.assertEqual(selected.profile, profile)
        self.assertEqual(len(profiles), 1)

    def test_no_match_returns_diagnostics(self):
        identity = _make_identity(fingerprint="other_fingerprint")
        profile = _make_profile()
        with patch(
            "apple.tools.code_signing.codesign_bundle._read_profiles_from_dir",
            return_value=[profile],
        ):
            selected, profiles, mismatches = _try_select_from_profiles_dir(
                profiles_dir=Path("/profiles"),
                info_plist_metadata=InfoPlistMetadata("com.company.app", None, False),
                identities=[identity],
                entitlements={},
                platform=ApplePlatform.ios_device,
                should_use_fast_provisioning_profile_parsing=False,
                strict_provisioning_profile_search=False,
                provisioning_profile_filter=None,
                no_check_certificates=False,
            )
        self.assertIsNone(selected)
        self.assertIsNotNone(profiles)
        self.assertEqual(len(profiles), 1)
        self.assertIsNotNone(mismatches)
        self.assertGreater(len(mismatches), 0)
