# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

from abc import ABCMeta, abstractmethod
from pathlib import Path
from typing import Union


class IReadProvisioningProfileCommandFactory(metaclass=ABCMeta):
    @abstractmethod
    def read_provisioning_profile_command(self, path: Path) -> list[Union[str, Path]]:
        raise NotImplementedError


# See `DEFAULT_READ_COMMAND` in `AppleConfig.java` in Buck v1
OPENSSL_PROV_PROFILE_READ_COMMAND = [
    "openssl",
    "smime",
    "-inform",
    "der",
    "-verify",
    "-noverify",
    "-nosigs",
    "-in",
]


class DefaultReadProvisioningProfileCommandFactory(
    IReadProvisioningProfileCommandFactory
):
    def read_provisioning_profile_command(self, path: Path) -> list[Union[str, Path]]:
        return OPENSSL_PROV_PROFILE_READ_COMMAND + [path]
