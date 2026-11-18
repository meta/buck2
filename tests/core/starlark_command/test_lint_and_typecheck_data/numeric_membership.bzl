# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def in_list(x: float) -> bool:
    return x in [1, 2]

def dict_index(d: dict[int, str], k: float) -> str:
    return d[k]

def in_dict(d: dict[int, str], k: float) -> bool:
    return k in d

def in_set(s: set[int], k: float) -> bool:
    return k in s
