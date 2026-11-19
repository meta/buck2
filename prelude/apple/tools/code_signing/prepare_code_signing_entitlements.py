# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import copy
import os
import plistlib
import tempfile
from pathlib import Path
from typing import Dict, List, Optional

from apple.tools.info_plist_processor.process import process as process_info_plist

from .provisioning_profile_metadata import ProvisioningProfileMetadata


def postprocess_entitlements(
    output_path: str,
    entitlements_suffixed_key_map: Optional[Dict[str, str]] = None,
    entitlements_removed_keys: Optional[List[str]] = None,
    entitlements_removed_values_map: Optional[Dict[str, List[str]]] = None,
) -> None:
    if (
        not entitlements_suffixed_key_map
        and not entitlements_removed_keys
        and not entitlements_removed_values_map
    ):
        return

    with open(output_path, "rb") as f:
        entitlements = plistlib.load(f)
    original = copy.deepcopy(entitlements)

    if entitlements_removed_keys:
        for key in entitlements_removed_keys:
            entitlements.pop(key, None)

    if entitlements_removed_values_map:
        for key, values_to_remove in entitlements_removed_values_map.items():
            if key in entitlements:
                current = entitlements[key]
                if isinstance(current, list):
                    entitlements[key] = [
                        v for v in current if v not in values_to_remove
                    ]
                elif isinstance(current, dict):
                    for v in values_to_remove:
                        current.pop(v, None)

    if entitlements_suffixed_key_map:
        for key, suffix in entitlements_suffixed_key_map.items():
            if key in entitlements:
                value = entitlements[key]
                if isinstance(value, str):
                    entitlements[key] = value + suffix
                elif isinstance(value, list):
                    entitlements[key] = [v + suffix for v in value]

    if entitlements != original:
        with open(output_path, "wb") as f:
            plistlib.dump(entitlements, f, fmt=plistlib.FMT_XML)


# Buck v1 corresponding code is in `ProvisioningProfileCopyStep::execute` in `ProvisioningProfileCopyStep.java`
def prepare_code_signing_entitlements(
    entitlements_path: Optional[Path],
    bundle_id: str,
    profile: ProvisioningProfileMetadata,
    tmp_dir: str,
    entitlements_suffixed_key_map: Optional[Dict[str, str]] = None,
    entitlements_removed_keys: Optional[List[str]] = None,
    entitlements_removed_values_map: Optional[Dict[str, List[str]]] = None,
) -> Path:
    fd, output_path = tempfile.mkstemp(dir=tmp_dir)
    with os.fdopen(fd, mode="wb") as output:
        if entitlements_path:
            with open(entitlements_path, "rb") as entitlements_file:
                process_info_plist(
                    input_file=entitlements_file,
                    output_file=output,
                    additional_keys=profile.get_mergeable_entitlements(),
                    output_format=plistlib.FMT_XML,
                )
        else:
            app_id = profile.get_app_id().team_id + "." + bundle_id
            entitlements = profile.get_mergeable_entitlements()
            entitlements["application-identifier"] = app_id
            entitlements["keychain-access-groups"] = [app_id]
            plistlib.dump(entitlements, output, fmt=plistlib.FMT_XML)

    postprocess_entitlements(
        output_path,
        entitlements_suffixed_key_map=entitlements_suffixed_key_map,
        entitlements_removed_keys=entitlements_removed_keys,
        entitlements_removed_values_map=entitlements_removed_values_map,
    )

    return Path(output_path)
