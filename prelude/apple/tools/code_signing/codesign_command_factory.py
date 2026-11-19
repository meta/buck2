# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import base64
import dataclasses
from abc import ABCMeta, abstractmethod
from pathlib import Path
from typing import Any, Dict, List, Optional, Union


@dataclasses.dataclass
class CodesignInvocation:
    path: Path
    identity_fingerprint: str
    entitlements: Optional[Path]
    codesign_args: List[str]
    extra_file_paths: Optional[List[Path]]

    def as_json(self, base_path: Path) -> Optional[Dict[str, Any]]:
        if not self.path.is_relative_to(base_path):
            # The dummy binary to be signed is not part of the bundle,
            # so it can be ignored
            return None

        invocation = {
            "path": str(self.path.relative_to(base_path)),
            "identity_fingerprint": self.identity_fingerprint,
            "codesign_args": self.codesign_args,
        }
        if self.entitlements:
            with open(self.entitlements, "rb") as entitlements_file:
                # We cannot rely on the presence of the entitlements file by
                # any consumers of the manifest because the file might not
                # be materialized or might be a temp file prepared by the
                # bundling script.
                entitlements_bytes = base64.b64encode(entitlements_file.read())
                invocation["entitlements"] = entitlements_bytes.decode("utf-8")
        if self.extra_file_paths:
            invocation["extra_file_paths"] = [
                str(extra_path) for extra_path in self.extra_file_paths
            ]
        return invocation


def generate_codesign_manifest(
    base_path: Path, codesign_invocations: list[CodesignInvocation]
) -> Dict[str, Any]:
    json_invocations = [
        invocation.as_json(base_path) for invocation in codesign_invocations
    ]
    valid_json_invocations = list(filter(lambda x: x is not None, json_invocations))
    return {
        "bundle_name": base_path.name,
        "invocations": valid_json_invocations,
    }


class ICodesignCommandFactory(metaclass=ABCMeta):
    @abstractmethod
    def codesign_command(
        self,
        path: Path,
        identity_fingerprint: str,
        entitlements: Optional[Path],
        codesign_args: List[str],
        extra_file_paths: Optional[List[Path]],
    ) -> List[Union[str, Path]]:
        raise NotImplementedError


class ManifestCodesignCommandFactory(ICodesignCommandFactory):
    def __init__(self, underlying: Optional[ICodesignCommandFactory]):
        self.underlying = underlying
        self.invocations = []

    def codesign_command(
        self,
        path: Path,
        identity_fingerprint: str,
        entitlements: Optional[Path],
        codesign_args: List[str],
        extra_file_paths: Optional[List[Path]],
    ) -> List[Union[str, Path]]:
        self.invocations.append(
            CodesignInvocation(
                path=path,
                identity_fingerprint=identity_fingerprint,
                entitlements=entitlements,
                codesign_args=codesign_args,
                extra_file_paths=extra_file_paths,
            )
        )
        if self.underlying:
            return self.underlying.codesign_command(
                path=path,
                identity_fingerprint=identity_fingerprint,
                entitlements=entitlements,
                codesign_args=codesign_args,
                extra_file_paths=extra_file_paths,
            )
        return ["/usr/bin/true"]

    def generate_codesign_manifest(self, base_path: Path) -> Dict[str, Any]:
        return generate_codesign_manifest(base_path, self.invocations)


class DefaultCodesignCommandFactory(ICodesignCommandFactory):
    codesign_tool: Path
    _command_args: List[str] = ["--force", "--sign"]

    def __init__(self, codesign_tool: Optional[Path]) -> None:
        self.codesign_tool = codesign_tool or Path("codesign")

    def codesign_command(
        self,
        path: Path,
        identity_fingerprint: str,
        entitlements: Optional[Path],
        codesign_args: List[str],
        extra_file_paths: Optional[List[Path]],
    ) -> List[Union[str, Path]]:
        if extra_file_paths:
            raise RuntimeError(
                f"Extra codesign paths unsupported for non-dry signing (path: `{path}`, extra paths: `{extra_file_paths}`)"
            )
        entitlements_args = ["--entitlements", entitlements] if entitlements else []
        return (
            [self.codesign_tool]
            + DefaultCodesignCommandFactory._command_args
            + [identity_fingerprint]
            + codesign_args
            + entitlements_args
            + [path]
        )


class DryRunCodesignCommandFactory(ICodesignCommandFactory):
    codesign_tool: Path
    subject_common_name: str

    def __init__(self, codesign_tool: Path, subject_common_name: str) -> None:
        self.codesign_tool = codesign_tool
        self.subject_common_name = subject_common_name

    def codesign_command(
        self,
        path: Path,
        identity_fingerprint: str,
        entitlements: Optional[Path],
        codesign_args: List[str],
        extra_file_paths: Optional[List[Path]],
    ) -> List[Union[str, Path]]:
        args = [
            path,
            "--identity",
            identity_fingerprint,
            "--subject-common-name",
            self.subject_common_name,
        ]
        if entitlements:
            args += ["--entitlements", entitlements] if entitlements else []
        if extra_file_paths:
            args += ["--extra-paths-to-sign"]
            args += extra_file_paths
        return [self.codesign_tool] + args
