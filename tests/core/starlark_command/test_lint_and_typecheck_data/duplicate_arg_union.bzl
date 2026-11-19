# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def f(a, b):
    return (a, b)

def h(*args, **kwargs):
    return (args, kwargs)

def use(c):
    k = f if c else h
    return k(1, a = 2)
