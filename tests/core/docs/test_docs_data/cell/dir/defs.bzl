# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
This is the summary for the module

And these are its details
"""

def bar(a, b: str, *, c: str, d: str = "some_default") -> list[str]:
    """
    This is the summary line for 'bar'

    These are the details that go below.
    We'll query for this symbol, and verify it matches json as expected
    Don't document 'd'

    Args:
        a: Docs for a
        b: Docs for b
        c: Docs for c
    """
    return ["a={a},  b={b},  c={c},  d={d}".format(a = a, b = b, c = c, d = d)]

def baz():
    """Simple docstring for baz"""
    pass

def quz():
    # No docstring
    pass

undocumented_variable = 5
