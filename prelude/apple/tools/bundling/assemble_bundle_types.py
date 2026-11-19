# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

from __future__ import annotations

import functools
from dataclasses import dataclass
from typing import Dict, List, Optional

from apple.tools.code_signing.codesign_bundle import CodesignConfiguration

from .incremental_state import IncrementalState


@functools.total_ordering
@dataclass
class BundleSpecItem:
    src: str
    # Should be bundle relative path, empty string means the root of the bundle
    dst: str
    codesign_on_copy: bool = False
    codesign_entitlements: Optional[str] = None
    codesign_flags_override: Optional[List[str]] = None
    extra_codesign_paths: Optional[List[str]] = None

    def __eq__(self: BundleSpecItem, other: Optional[BundleSpecItem]) -> bool:
        return (
            other is not None
            and self.src == other.src
            and self.dst == other.dst
            and self.codesign_on_copy == other.codesign_on_copy
            and self.codesign_entitlements == other.codesign_entitlements
            and self.codesign_flags_override == other.codesign_flags_override
            and self.extra_codesign_paths == other.extra_codesign_paths
        )

    def __ne__(self: BundleSpecItem, other: BundleSpecItem) -> bool:
        return not self.__eq__(other)

    def __hash__(self: BundleSpecItem) -> int:
        return hash(
            (
                self.src,
                self.dst,
                self.codesign_on_copy,
                self.codesign_entitlements,
                (
                    tuple(self.codesign_flags_override)
                    if self.codesign_flags_override is not None
                    else hash(None)
                ),
                (
                    tuple(self.extra_codesign_paths)
                    if self.extra_codesign_paths is not None
                    else hash(None)
                ),
            )
        )

    def _ordering_key(self: BundleSpecItem) -> tuple:
        # `None` sorts before every value of an optional field.
        return (
            self.src,
            self.dst,
            self.codesign_on_copy,
            self.codesign_entitlements is not None,
            self.codesign_entitlements or "",
            self.codesign_flags_override is not None,
            tuple(self.codesign_flags_override or []),
            self.extra_codesign_paths is not None,
            tuple(self.extra_codesign_paths or []),
        )

    def __lt__(self: BundleSpecItem, other: BundleSpecItem) -> bool:
        return self._ordering_key() < other._ordering_key()


@dataclass
class IncrementalContext:
    """
    Additional data you need to bundle incrementally (extra vs when non-incrementally).
    """

    # Maps buck-project relative path to hash digest of the input file.
    metadata: Dict[str, str]
    # Present when there is a valid incremental state on disk (i.e. previous build produced it).
    state: Optional[IncrementalState]
    codesigned: bool
    codesign_configuration: Optional[CodesignConfiguration]
    codesign_identity: Optional[str]
    codesign_arguments: List[str]
    versioned_if_macos: bool
