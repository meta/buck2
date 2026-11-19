# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import datetime
import unittest
from pathlib import Path

from apple.tools.code_signing.identity import CodeSigningIdentity
from apple.tools.code_signing.info_plist_metadata import InfoPlistMetadata
from apple.tools.code_signing.provisioning_profile_metadata import (
    ProvisioningProfileMetadata,
)
from apple.tools.code_signing.provisioning_profile_selection import (
    SelectedProvisioningProfileInfo,
)
from apple.tools.code_signing.signing_context_types import (
    AdhocSigningContext,
    SigningContextWithProfileSelection,
)

from .signing_info import build_signing_info


class TestSigningInfo(unittest.TestCase):
    def test_no_signing_context(self):
        info = build_signing_info(None, None)
        self.assertEqual(info, {})

    def test_adhoc_without_profile(self):
        ctx = AdhocSigningContext(codesign_identity="-", profile_selection_context=None)
        info = build_signing_info(ctx, "-")
        self.assertEqual(info["codesign_type"], "adhoc")
        self.assertEqual(info["codesign_identity"], "-")
        self.assertNotIn("provisioning_profile", info)

    def test_distribution_with_devices_list(self):
        expiration = datetime.datetime(2025, 1, 1, 12, 0, 0)
        profile = ProvisioningProfileMetadata(
            file_path=Path("/tmp/my.mobileprovision"),
            uuid="UUID-123",
            expiration_date=expiration,
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(["ABC"]),
            entitlements={},
            creation_date=None,
            provisions_all_devices=False,
            provisioned_devices=["dev1", "dev2"],
        )
        identity = CodeSigningIdentity(
            fingerprint="ABC", subject_common_name="Apple Dev"
        )
        selected = SelectedProvisioningProfileInfo(profile=profile, identity=identity)
        metadata = InfoPlistMetadata(
            bundle_id="com.test", bundle_type="APPL", is_watchos_app=False
        )
        ctx = SigningContextWithProfileSelection(
            info_plist_source=Path("/tmp/Info.plist"),
            info_plist_destination=Path("Info.plist"),
            info_plist_metadata=metadata,
            selected_profile_info=selected,
        )
        info = build_signing_info(ctx, "ABC")
        self.assertEqual(info["codesign_type"], "distribution")
        self.assertEqual(info["codesign_identity"], "ABC")
        self.assertEqual(info["provisioning_profile"]["uuid"], "UUID-123")
        self.assertEqual(
            info["provisioning_profile"]["file_name"], "my.mobileprovision"
        )
        self.assertEqual(info["provisioning_profile"]["provisioned_devices"], "list")
        self.assertEqual(info["signing_certificate"]["fingerprint"], "ABC")

    def test_provisioned_devices_all_and_none(self):
        expiration = datetime.datetime(2025, 1, 1, 12, 0, 0)

        profile_all = ProvisioningProfileMetadata(
            file_path=Path("/tmp/all.mobileprovision"),
            uuid="UUID-ALL",
            expiration_date=expiration,
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(["ABC"]),
            entitlements={},
            creation_date=None,
            provisions_all_devices=True,
            provisioned_devices=None,
        )
        identity = CodeSigningIdentity(fingerprint="ABC", subject_common_name="Dev")
        selected_all = SelectedProvisioningProfileInfo(
            profile=profile_all, identity=identity
        )
        metadata = InfoPlistMetadata(
            bundle_id="com.test", bundle_type="APPL", is_watchos_app=False
        )
        ctx_all = SigningContextWithProfileSelection(
            info_plist_source=Path("/tmp/Info.plist"),
            info_plist_destination=Path("Info.plist"),
            info_plist_metadata=metadata,
            selected_profile_info=selected_all,
        )
        info_all = build_signing_info(ctx_all, "ABC")
        self.assertEqual(info_all["provisioning_profile"]["provisioned_devices"], "all")

        profile_none = ProvisioningProfileMetadata(
            file_path=Path("/tmp/none.mobileprovision"),
            uuid="UUID-NONE",
            expiration_date=expiration,
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(["ABC"]),
            entitlements={},
            creation_date=None,
            provisions_all_devices=False,
            provisioned_devices=None,
        )
        selected_none = SelectedProvisioningProfileInfo(
            profile=profile_none, identity=identity
        )
        ctx_none = SigningContextWithProfileSelection(
            info_plist_source=Path("/tmp/Info.plist"),
            info_plist_destination=Path("Info.plist"),
            info_plist_metadata=metadata,
            selected_profile_info=selected_none,
        )
        info_none = build_signing_info(ctx_none, "ABC")
        self.assertEqual(
            info_none["provisioning_profile"]["provisioned_devices"], "none"
        )
