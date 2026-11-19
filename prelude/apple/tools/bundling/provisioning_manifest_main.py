# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import argparse
import base64
import json
from pathlib import Path

from apple.tools.code_signing.signing_context_types import (
    selection_profile_context_from_signing_context,
)

from .signing_context_data import load_signing_context_data_from_file


def _main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate a provisioning manifest from a signing context.",
    )
    parser.add_argument(
        "--output",
        required=True,
        type=Path,
        help="Path to the output JSON file.",
    )
    parser.add_argument(
        "--signing-context-path",
        type=Path,
        required=True,
        help="Path to the precomputed signing context JSON.",
    )

    args = parser.parse_args()
    signing_context_data = load_signing_context_data_from_file(
        args.signing_context_path
    )
    selection_profile_context = selection_profile_context_from_signing_context(
        signing_context_data.signing_context
    )

    with open(args.output, "w") as output_file:
        signing_context_json_obj = {
            "version": 1,
        }

        if signing_context_data.selected_identity:
            # Ad hoc and Developer ID signing use the human-readable identity;
            # profile-based signing uses the certificate fingerprint.
            signing_context_json_obj["codesign_identity"] = (
                signing_context_data.selected_identity
            )

        if selection_profile_context:
            selected_profile_info = selection_profile_context.selected_profile_info
            profile_metadata = selected_profile_info.profile
            profile_data = signing_context_data.provisioning_profile_data
            if profile_data is None:
                raise ValueError("Signing context is missing profile data")

            signing_context_json_obj["provisioning_profile"] = {
                "uuid": profile_metadata.uuid,
                "identity": {
                    "fingerprint": selected_profile_info.identity.fingerprint,
                    "subject_common_name": selected_profile_info.identity.subject_common_name,
                },
                "file_name": profile_metadata.file_path.name,
                "file_data_base64": base64.b64encode(profile_data).decode(),
            }

        json.dump(
            signing_context_json_obj,
            output_file,
            indent=4,
        )


if __name__ == "__main__":
    _main()
