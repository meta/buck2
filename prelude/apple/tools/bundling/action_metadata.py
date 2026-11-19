# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import json
import os
from io import TextIOBase
from pathlib import Path
from typing import Dict, Optional

_METADATA_VERSION = 1


def parse_action_metadata(data: TextIOBase) -> Optional[Dict[str, str]]:
    """
    Returns:
        Mapping from project relative path to hash digest for every file present action metadata.
    """
    metadata = json.load(data)
    version = metadata["version"]
    if version != _METADATA_VERSION:
        raise RuntimeError(
            f"Expected metadata version to be `{_METADATA_VERSION}` got `{version}`."
        )
    return {item["path"]: item["digest"] for item in metadata["digests"]}


def action_metadata_if_present(
    environment_variable_key: str,
) -> Optional[Dict[str, str]]:
    """
    Returns:
        Mapping from project relative path to hash digest for every file present action metadata.
    """
    environment_variable = os.getenv(environment_variable_key)
    if environment_variable is None:
        return None
    path = Path(environment_variable)
    if not path.exists():
        raise RuntimeError(
            "Expected file with action metadata to exist given related environment variable is set."
        )
    else:
        with path.open() as f:
            return parse_action_metadata(f)
