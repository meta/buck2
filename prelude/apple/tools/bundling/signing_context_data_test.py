# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import datetime
import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

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

from .signing_context_data import (
    deserialize_signing_context_data,
    serialize_signing_context_data,
)


class TestSigningContextData(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = TemporaryDirectory()
        self.profile_path = Path(self.temp_dir.name) / "profile.mobileprovision"
        self.profile_path.write_bytes(b"profile data")

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def test_null_context(self):
        data = serialize_signing_context_data(None, None)
        context_data = deserialize_signing_context_data(data)
        self.assertIsNone(context_data.signing_context)
        self.assertIsNone(context_data.selected_identity)
        self.assertIsNone(context_data.provisioning_profile_data)

    def test_adhoc_without_profile(self):
        ctx = AdhocSigningContext(codesign_identity="-", profile_selection_context=None)
        data = serialize_signing_context_data(ctx, "-")
        json_str = json.dumps(data)
        loaded = json.loads(json_str)
        context_data = deserialize_signing_context_data(loaded)
        ctx2 = context_data.signing_context
        self.assertEqual(context_data.selected_identity, "-")
        self.assertIsNone(context_data.provisioning_profile_data)
        assert isinstance(ctx2, AdhocSigningContext)
        self.assertEqual(ctx2.codesign_identity, "-")
        self.assertIsNone(ctx2.profile_selection_context)

    def test_distribution_context(self):
        expiration = datetime.datetime(2025, 1, 1, 12, 0, 0)
        creation = datetime.datetime(2024, 1, 1, 12, 0, 0)
        profile = ProvisioningProfileMetadata(
            file_path=self.profile_path,
            uuid="UUID-1234",
            expiration_date=expiration,
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(
                ["ABCDEF1234567890ABCDEF1234567890ABCDEF12"]
            ),
            entitlements={"application-identifier": "TEAM.com.test.app"},
            creation_date=creation,
            provisions_all_devices=False,
            provisioned_devices=["device1"],
        )
        identity = CodeSigningIdentity(
            fingerprint="ABCDEF1234567890ABCDEF1234567890ABCDEF12",
            subject_common_name="Apple Development: Test",
        )
        selected_info = SelectedProvisioningProfileInfo(
            profile=profile, identity=identity
        )
        info_plist_metadata = InfoPlistMetadata(
            bundle_id="com.test.app", bundle_type="APPL", is_watchos_app=False
        )
        ctx = SigningContextWithProfileSelection(
            info_plist_source=Path("/tmp/Info.plist"),
            info_plist_destination=Path("Info.plist"),
            info_plist_metadata=info_plist_metadata,
            selected_profile_info=selected_info,
        )
        data = serialize_signing_context_data(ctx, identity.fingerprint)
        json_str = json.dumps(data)
        loaded = json.loads(json_str)
        context_data = deserialize_signing_context_data(loaded)
        ctx2 = context_data.signing_context
        assert isinstance(ctx2, SigningContextWithProfileSelection)
        self.assertEqual(context_data.selected_identity, identity.fingerprint)
        self.assertEqual(context_data.provisioning_profile_data, b"profile data")
        self.assertEqual(ctx2.info_plist_source, Path("/tmp/Info.plist"))
        self.assertEqual(ctx2.info_plist_destination, Path("Info.plist"))
        self.assertEqual(ctx2.info_plist_metadata.bundle_id, "com.test.app")
        self.assertEqual(ctx2.selected_profile_info.profile.uuid, "UUID-1234")
        self.assertEqual(
            ctx2.selected_profile_info.profile.file_path,
            self.profile_path,
        )
        self.assertEqual(
            ctx2.selected_profile_info.identity.fingerprint, identity.fingerprint
        )
        self.assertEqual(
            ctx2.selected_profile_info.profile.provisions_all_devices, False
        )
        self.assertEqual(
            ctx2.selected_profile_info.profile.provisioned_devices, ["device1"]
        )

    def test_adhoc_with_profile(self):
        expiration = datetime.datetime(2025, 1, 1, 12, 0, 0)
        profile = ProvisioningProfileMetadata(
            file_path=self.profile_path,
            uuid="UUID-5678",
            expiration_date=expiration,
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(
                ["AAAABBBBCCCCDDDDEEEEFFFF0000111122223333"]
            ),
            entitlements={},
            creation_date=None,
            provisions_all_devices=True,
            provisioned_devices=None,
        )
        identity = CodeSigningIdentity(
            fingerprint="AAAABBBBCCCCDDDDEEEEFFFF0000111122223333",
            subject_common_name="Apple Development: Adhoc",
        )
        selected_info = SelectedProvisioningProfileInfo(
            profile=profile, identity=identity
        )
        info_plist_metadata = InfoPlistMetadata(
            bundle_id="com.test.app", bundle_type="APPL", is_watchos_app=False
        )
        distribution_ctx = SigningContextWithProfileSelection(
            info_plist_source=Path("/tmp/Info.plist"),
            info_plist_destination=Path("Info.plist"),
            info_plist_metadata=info_plist_metadata,
            selected_profile_info=selected_info,
        )
        adhoc_ctx = AdhocSigningContext(
            codesign_identity="Apple Development: Adhoc",
            profile_selection_context=distribution_ctx,
        )
        data = serialize_signing_context_data(adhoc_ctx, "Apple Development: Adhoc")
        json_str = json.dumps(data)
        loaded = json.loads(json_str)
        context_data = deserialize_signing_context_data(loaded)
        ctx2 = context_data.signing_context
        assert isinstance(ctx2, AdhocSigningContext)
        self.assertEqual(ctx2.codesign_identity, "Apple Development: Adhoc")
        assert ctx2.profile_selection_context is not None
        self.assertEqual(
            ctx2.profile_selection_context.selected_profile_info.profile.uuid,
            "UUID-5678",
        )
        self.assertEqual(context_data.selected_identity, "Apple Development: Adhoc")
        self.assertEqual(context_data.provisioning_profile_data, b"profile data")

    def test_version_mismatch_raises(self):
        bad_data = {
            "version": 999,
            "selected_identity": None,
            "signing_context": None,
            "provisioning_profile_data_base64": None,
        }
        with self.assertRaises(ValueError) as cm:
            deserialize_signing_context_data(bad_data)

        msg = str(cm.exception)
        self.assertIn("Expected signing context version", msg)
        self.assertIn("999", msg)

        bad_data_old = {
            "version": 2,
            "selected_identity": "-",
            "signing_context": {
                "kind": "adhoc",
                "adhoc": {"codesign_identity": "-", "profile_selection_context": None},
            },
            "provisioning_profile_data_base64": None,
        }
        with self.assertRaises(ValueError):
            deserialize_signing_context_data(bad_data_old)

    def test_profile_context_requires_profile_data(self):
        profile = ProvisioningProfileMetadata(
            file_path=self.profile_path,
            uuid="UUID",
            expiration_date=datetime.datetime(2025, 1, 1),
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(["ABC"]),
            entitlements={},
        )
        context = SigningContextWithProfileSelection(
            info_plist_source=Path("/tmp/Info.plist"),
            info_plist_destination=Path("Info.plist"),
            info_plist_metadata=InfoPlistMetadata("com.test", "APPL", False),
            selected_profile_info=SelectedProvisioningProfileInfo(
                profile=profile,
                identity=CodeSigningIdentity("ABC", "Developer"),
            ),
        )
        data = serialize_signing_context_data(context, "ABC")
        data["provisioning_profile_data_base64"] = None

        with self.assertRaisesRegex(ValueError, "must be present exactly"):
            deserialize_signing_context_data(data)
