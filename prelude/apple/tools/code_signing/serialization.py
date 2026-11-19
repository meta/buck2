# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from typing import cast


def expect_dict(value: object, path: str) -> dict[str, object]:
    if not isinstance(value, dict) or not all(
        isinstance(key, str) for key in value.keys()
    ):
        raise ValueError(f"{path} must be an object")
    return cast(dict[str, object], value)


def expect_keys(
    data: dict[str, object],
    path: str,
    expected: frozenset[str],
) -> None:
    actual = frozenset(data)
    if actual != expected:
        missing = sorted(expected - actual)
        unexpected = sorted(actual - expected)
        raise ValueError(
            f"{path} has invalid keys: missing={missing}, unexpected={unexpected}"
        )


def expect_str(value: object, path: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{path} must be a string")
    return value


def expect_optional_str(value: object, path: str) -> str | None:
    if value is None:
        return None
    return expect_str(value, path)


def expect_bool(value: object, path: str) -> bool:
    if not isinstance(value, bool):
        raise ValueError(f"{path} must be a boolean")
    return value


def expect_optional_bool(value: object, path: str) -> bool | None:
    if value is None:
        return None
    return expect_bool(value, path)


def expect_str_list(value: object, path: str) -> list[str]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise ValueError(f"{path} must be a list of strings")
    return cast(list[str], value)


def expect_optional_str_list(value: object, path: str) -> list[str] | None:
    if value is None:
        return None
    return expect_str_list(value, path)
