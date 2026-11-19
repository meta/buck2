# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import base64
import binascii
import json
from dataclasses import dataclass
from pathlib import Path

from apple.tools.code_signing.serialization import (
    expect_dict,
    expect_keys,
    expect_optional_str,
    expect_str,
)
from apple.tools.code_signing.signing_context_types import (
    AdhocSigningContext,
    selection_profile_context_from_signing_context,
    SigningContextWithProfileSelection,
)

_VERSION = 1

SigningContext = AdhocSigningContext | SigningContextWithProfileSelection


@dataclass(frozen=True)
class SigningContextData:
    signing_context: SigningContext | None
    selected_identity: str | None
    provisioning_profile_data: bytes | None


def serialize_signing_context_data(
    signing_context: SigningContext | None,
    selected_identity: str | None,
) -> dict[str, object]:
    profile_context = selection_profile_context_from_signing_context(signing_context)
    profile_data = (
        profile_context.selected_profile_info.profile.file_path.read_bytes()
        if profile_context is not None
        else None
    )
    return {
        "version": _VERSION,
        "selected_identity": selected_identity,
        "signing_context": _serialize_signing_context(signing_context),
        "provisioning_profile_data_base64": (
            base64.b64encode(profile_data).decode()
            if profile_data is not None
            else None
        ),
    }


def _serialize_signing_context(
    signing_context: SigningContext | None,
) -> dict[str, object] | None:
    if signing_context is None:
        return None
    if isinstance(signing_context, SigningContextWithProfileSelection):
        return {"kind": "distribution", "distribution": signing_context.to_dict()}
    if isinstance(signing_context, AdhocSigningContext):
        return {"kind": "adhoc", "adhoc": signing_context.to_dict()}
    raise ValueError(f"Unexpected signing context type: {type(signing_context)}")


def deserialize_signing_context_data(value: object) -> SigningContextData:
    data = expect_dict(value, "SigningContextData")
    expect_keys(
        data,
        "SigningContextData",
        frozenset(
            {
                "version",
                "selected_identity",
                "signing_context",
                "provisioning_profile_data_base64",
            }
        ),
    )
    version = data["version"]
    if type(version) is not int or version != _VERSION:
        raise ValueError(f"Expected signing context version {_VERSION}, got {version}")

    signing_context = _deserialize_signing_context(data["signing_context"])
    profile_data = _decode_profile_data(data["provisioning_profile_data_base64"])
    profile_context = selection_profile_context_from_signing_context(signing_context)
    if (profile_context is None) != (profile_data is None):
        raise ValueError(
            "provisioning_profile_data_base64 must be present exactly when a "
            "provisioning profile is selected"
        )

    return SigningContextData(
        signing_context=signing_context,
        selected_identity=expect_optional_str(
            data["selected_identity"], "SigningContextData.selected_identity"
        ),
        provisioning_profile_data=profile_data,
    )


def _deserialize_signing_context(value: object) -> SigningContext | None:
    if value is None:
        return None
    data = expect_dict(value, "SigningContextData.signing_context")
    kind = expect_str(data.get("kind"), "SigningContextData.signing_context.kind")
    if kind == "distribution":
        expect_keys(
            data,
            "SigningContextData.signing_context",
            frozenset({"kind", "distribution"}),
        )
        return SigningContextWithProfileSelection.from_dict(data["distribution"])
    if kind == "adhoc":
        expect_keys(
            data,
            "SigningContextData.signing_context",
            frozenset({"kind", "adhoc"}),
        )
        return AdhocSigningContext.from_dict(data["adhoc"])
    raise ValueError(f"Unknown signing context kind: {kind}")


def _decode_profile_data(value: object) -> bytes | None:
    encoded = expect_optional_str(
        value, "SigningContextData.provisioning_profile_data_base64"
    )
    if encoded is None:
        return None
    try:
        return base64.b64decode(encoded, validate=True)
    except binascii.Error as error:
        raise ValueError("Invalid provisioning profile base64 data") from error


def write_signing_context_data_to_file(
    path: Path,
    signing_context: SigningContext | None,
    selected_identity: str | None,
) -> None:
    data = serialize_signing_context_data(signing_context, selected_identity)
    with path.open("w") as output:
        json.dump(data, output, indent=4)


def load_signing_context_data_from_file(path: Path) -> SigningContextData:
    with path.open() as source:
        return deserialize_signing_context_data(json.load(source))
