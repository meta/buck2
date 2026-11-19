# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


from __future__ import annotations

from pathlib import Path
from typing import Any

from apple.tools.code_signing.signing_context_types import (
    AdhocSigningContext,
    selection_profile_context_from_signing_context,
    SigningContextWithProfileSelection,
)

from .signing_context_data import load_signing_context_data_from_file


def build_signing_info(
    signing_context: AdhocSigningContext | SigningContextWithProfileSelection | None,
    selected_identity: str | None,
) -> dict[str, Any]:
    if signing_context is None:
        return {}

    if isinstance(signing_context, AdhocSigningContext):
        codesign_type = "adhoc"
    elif isinstance(signing_context, SigningContextWithProfileSelection):
        codesign_type = "distribution"
    else:
        raise RuntimeError(f"Unexpected signing context type: {type(signing_context)}")

    signing_info: dict[str, Any] = {
        "codesign_type": codesign_type,
    }

    if selected_identity:
        signing_info["codesign_identity"] = selected_identity

    selection_profile_context = selection_profile_context_from_signing_context(
        signing_context
    )

    if selection_profile_context:
        selected_profile_info = selection_profile_context.selected_profile_info
        profile_metadata = selected_profile_info.profile
        if profile_metadata.provisioned_devices is not None:
            provisioned_devices = "list"
        elif profile_metadata.provisions_all_devices:
            provisioned_devices = "all"
        else:
            provisioned_devices = "none"

        signing_info["provisioning_profile"] = {
            "uuid": profile_metadata.uuid,
            "file_name": profile_metadata.file_path.name,
            "provisioned_devices": provisioned_devices,
        }
        signing_info["signing_certificate"] = {
            "fingerprint": selected_profile_info.identity.fingerprint,
            "subject_common_name": selected_profile_info.identity.subject_common_name,
        }

    return signing_info


def build_signing_info_from_file(path: Path) -> dict[str, Any]:
    signing_context_data = load_signing_context_data_from_file(path)
    return build_signing_info(
        signing_context_data.signing_context,
        signing_context_data.selected_identity,
    )
