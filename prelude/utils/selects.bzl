# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:type_defs.bzl", "type_utils")

def _apply(obj, function):
    """
    Runs select_map(obj, function, recurse=True)
    """
    return select_map(obj, function, recurse = True)

def _tie_n_impl_inner(objs, pvals, val):
    return _tie_n_impl(objs[1:], pvals + [val])

def _tie_n_impl(objs, pvals):
    if not objs:
        return tuple(pvals)

    return select_map(
        objs[0],
        partial(_tie_n_impl_inner, objs, pvals),
        recurse = True,
    )

def _tie_n(*objs):
    """
    Combine separate select objects into a single select with all values of
    the select "tied" into n-tuples.

    Arguments:
        objs: Objects which may be behind (possible nested) `select`s.

    Returns: A `select` resolving to an n-tuple of the passed in object
    """
    return _tie_n_impl(objs, [])

def _apply_n_inner(func, vals):
    return func(*vals)

def _apply_n(objs, func):
    """
    Return a new `select` formed by applying the given function to all possible
    combinations of the given select objects.
    """
    return select_map(
        _tie_n(*objs),
        # Unpack n-tuple and call user-supplied function.
        partial(_apply_n_inner, func),
        recurse = True,
    )

selects = struct(
    apply = _apply,
    apply_n = _apply_n,
    is_select = type_utils.is_select,
)
