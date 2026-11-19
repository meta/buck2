# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

from __future__ import annotations

import subprocess
from abc import ABCMeta, abstractmethod
from typing import List

from .identity import CodeSigningIdentity


class IListCodesignIdentities(metaclass=ABCMeta):
    @abstractmethod
    def list_codesign_identities(self) -> List[CodeSigningIdentity]:
        raise NotImplementedError

    @abstractmethod
    def raw_command(self) -> str:
        raise NotImplementedError


class ListCodesignIdentities(IListCodesignIdentities):
    _default_command = ["security", "find-identity", "-v", "-p", "codesigning"]

    def __init__(self, command: List[str]) -> None:
        self.command = command

    @classmethod
    def default(cls) -> IListCodesignIdentities:
        return cls(cls._default_command)

    @classmethod
    def override(cls, command: List[str]) -> IListCodesignIdentities:
        return cls(command)

    def list_codesign_identities(self) -> List[CodeSigningIdentity]:
        return _list_identities(self.command)

    def raw_command(self) -> str:
        return " ".join(self.command)


def _list_identities(
    command: List[str],
) -> List[CodeSigningIdentity]:
    output = subprocess.check_output(
        command,
        encoding="utf-8",
    )
    return CodeSigningIdentity.parse_security_stdout(output)


class AdHocListCodesignIdentities(IListCodesignIdentities):
    def __init__(
        self, original: IListCodesignIdentities, subject_common_name: str
    ) -> None:
        self.original = original
        self.subject_common_name = subject_common_name

    def list_codesign_identities(self) -> List[CodeSigningIdentity]:
        unfiltered_identities = self.original.list_codesign_identities()
        identity = next(
            (
                i
                for i in unfiltered_identities
                if i.subject_common_name == self.subject_common_name
            ),
            None,
        )
        if not identity:
            raise RuntimeError(
                f"No identity found with subject common name `{self.subject_common_name}`"
            )
        return [identity]

    def raw_command(self) -> str:
        return self.original.raw_command()
