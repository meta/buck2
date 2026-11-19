# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Optional, Union

from .identity import CodeSigningIdentity
from .info_plist_metadata import InfoPlistMetadata
from .provisioning_profile_selection import SelectedProvisioningProfileInfo
from .serialization import expect_dict, expect_keys, expect_str


@dataclass
class SigningContextWithProfileSelection:
    info_plist_source: Path
    info_plist_destination: Path
    info_plist_metadata: InfoPlistMetadata
    selected_profile_info: SelectedProvisioningProfileInfo

    def to_dict(self) -> dict[str, object]:
        return {
            "info_plist_source": str(self.info_plist_source),
            "info_plist_destination": str(self.info_plist_destination),
            "info_plist_metadata": self.info_plist_metadata.to_dict(),
            "selected_profile_info": self.selected_profile_info.to_dict(),
        }

    @staticmethod
    def from_dict(value: object) -> SigningContextWithProfileSelection:
        data = expect_dict(value, "SigningContextWithProfileSelection")
        expect_keys(
            data,
            "SigningContextWithProfileSelection",
            frozenset(
                {
                    "info_plist_source",
                    "info_plist_destination",
                    "info_plist_metadata",
                    "selected_profile_info",
                }
            ),
        )
        return SigningContextWithProfileSelection(
            info_plist_source=Path(
                expect_str(
                    data["info_plist_source"],
                    "SigningContextWithProfileSelection.info_plist_source",
                )
            ),
            info_plist_destination=Path(
                expect_str(
                    data["info_plist_destination"],
                    "SigningContextWithProfileSelection.info_plist_destination",
                )
            ),
            info_plist_metadata=InfoPlistMetadata.from_dict(
                data["info_plist_metadata"]
            ),
            selected_profile_info=SelectedProvisioningProfileInfo.from_dict(
                data["selected_profile_info"]
            ),
        )


@dataclass
class AdhocSigningContext:
    codesign_identity: str
    profile_selection_context: Optional[SigningContextWithProfileSelection]

    def __init__(
        self,
        codesign_identity: Optional[str] = None,
        profile_selection_context: Optional[SigningContextWithProfileSelection] = None,
    ) -> None:
        self.codesign_identity = codesign_identity or "-"
        self.profile_selection_context = profile_selection_context

    def identity(self) -> CodeSigningIdentity:
        if self.profile_selection_context:
            return self.profile_selection_context.selected_profile_info.identity
        return CodeSigningIdentity(
            fingerprint=self.codesign_identity,
            subject_common_name="",
        )

    def to_dict(self) -> dict[str, object]:
        return {
            "codesign_identity": self.codesign_identity,
            "profile_selection_context": (
                self.profile_selection_context.to_dict()
                if self.profile_selection_context
                else None
            ),
        }

    @staticmethod
    def from_dict(value: object) -> AdhocSigningContext:
        data = expect_dict(value, "AdhocSigningContext")
        expect_keys(
            data,
            "AdhocSigningContext",
            frozenset({"codesign_identity", "profile_selection_context"}),
        )
        psc_data = data["profile_selection_context"]
        psc = (
            SigningContextWithProfileSelection.from_dict(psc_data)
            if psc_data is not None
            else None
        )
        return AdhocSigningContext(
            codesign_identity=expect_str(
                data["codesign_identity"], "AdhocSigningContext.codesign_identity"
            ),
            profile_selection_context=psc,
        )


def selection_profile_context_from_signing_context(
    signing_context: Optional[
        Union[AdhocSigningContext, SigningContextWithProfileSelection]
    ],
) -> Optional[SigningContextWithProfileSelection]:
    if signing_context:
        if isinstance(signing_context, SigningContextWithProfileSelection):
            selection_profile_context = signing_context
        elif isinstance(signing_context, AdhocSigningContext):
            selection_profile_context = signing_context.profile_selection_context
        else:
            raise RuntimeError(
                f"Unexpected type of signing context `{type(signing_context)}`"
            )
        return selection_profile_context
    return None
