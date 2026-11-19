# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:selects.bzl", _prelude_selects = "selects")

def _cond(condition: str) -> Select:
    """Returns a select that evaluates to True if the condition is met.

    Useful for terse logical conditions.

    For example,
    ```
    selects.if_(selects.and(selects.cond("//config:linux"), selects.cond("//config:clang")), ["-g1"], ["-g2"])
    ```
    """
    return select({
        "DEFAULT": False,
        condition: True,
    })

def _and(val, *vals):
    if not vals:
        return val
    return _prelude_selects.apply(
        val,
        lambda val: val and _and(*vals),
    )

def _or(val, *vals):
    if not vals:
        return val
    return _prelude_selects.apply(
        val,
        lambda val: val or _or(*vals),
    )

def _if(cond, a, b):
    return _prelude_selects.apply(
        cond,
        native.partial(_if_inner, a = a, b = b),
    )

def _if_inner(cond, a, b):
    return a if cond else b

def _with_or(conditions):
    """Expands tuple keys into one select entry per condition.

    Mirrors the internal selects.with_or() used by exported BUCK files (e.g.
    folly's defs.bzl): `{("//c:a", "//c:b"): v}` becomes a select in which
    both conditions map to the same value.
    """
    expanded = {}
    for conditions_key, value in conditions.items():
        if type(conditions_key) == type(()):
            for condition in conditions_key:
                expanded[condition] = value
        else:
            expanded[conditions_key] = value
    return select(expanded)

selects = struct(
    and_ = _and,
    cond = _cond,
    or_ = _or,
    if_ = _if,
    with_or = _with_or,
)
