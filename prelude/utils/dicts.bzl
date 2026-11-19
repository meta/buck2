# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:expect.bzl", "expect")

_DEFAULT_FMT = 'found different values for key "{}": {} != {}'

def update_x(dst: dict[typing.Any, typing.Any], k: typing.Any, v: typing.Any, fmt = _DEFAULT_FMT):
    p = dst.setdefault(k, v)
    expect(p == v, fmt, k, p, v)

def merge_x(dst: dict[typing.Any, typing.Any], src: dict[typing.Any, typing.Any], fmt = _DEFAULT_FMT):
    for k, v in src.items():
        update_x(dst, k, v, fmt = fmt)

def flatten_x(ds: list[dict[typing.Any, typing.Any]], fmt = _DEFAULT_FMT) -> dict[typing.Any, typing.Any]:
    out = {}
    for d in ds:
        merge_x(out, d, fmt = fmt)
    return out
