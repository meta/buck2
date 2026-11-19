# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A test to ensure we coerce large numbers to bigint when resolving attrs.

MirrorInfo = provider(fields = ["info"])

def _assert_eq(a, b):
    if a != b:
        fail("Expected {} == {}".format(a, b))

def _mirror_impl(ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(), MirrorInfo(info = ctx.attrs)]

_mirror = rule(
    impl = _mirror_impl,
    attrs = {
        "any": attrs.any(),
        "int": attrs.int(),
        "string": attrs.string(),
    },
)

def _int8_num_impl(ctx: AnalysisContext) -> Promise:
    def f(providers):
        res = providers[MirrorInfo].info
        _assert_eq(res.any, -42)
        _assert_eq(res.int, -42)
        _assert_eq(res.string, "-42")
        return [DefaultInfo()]

    at = {
        "any": -21 * 2,
        "int": -21 * 2,
        "string": str(-21 * 2),
    }
    return ctx.actions.anon_target(_mirror, at).promise.map(f)

_int8_num = rule(impl = _int8_num_impl, attrs = {})

def _int32_num_impl(ctx: AnalysisContext) -> Promise:
    """1500000000 fits in int32, 3000000000 does not"""

    def f(providers):
        res = providers[MirrorInfo].info
        _assert_eq(res.any, -3000000000)
        _assert_eq(res.int, -3000000000)
        _assert_eq(res.string, "-3000000000")
        return [DefaultInfo()]

    at = {
        "any": -1500000000 * 2,
        "int": -1500000000 * 2,
        "string": str(-1500000000 * 2),
    }
    return ctx.actions.anon_target(_mirror, at).promise.map(f)

_int32_num = rule(impl = _int32_num_impl, attrs = {})

def _int64_num_impl(ctx: AnalysisContext) -> Promise:
    """2^40 = 1099511627776, 2^20 = 1048576, 2^60 = 2^40 * 2^20 = 1152921504606846976"""

    def f(providers):
        res = providers[MirrorInfo].info
        _assert_eq(res.any, -1152921504606846976)
        _assert_eq(res.int, -1152921504606846976)
        _assert_eq(res.string, "-1152921504606846976")
        return [DefaultInfo()]

    at = {
        "any": -1099511627776 * 1048576,
        "int": -1099511627776 * 1048576,
        "string": str(-1099511627776 * 1048576),
    }
    return ctx.actions.anon_target(_mirror, at).promise.map(f)

_int64_num = rule(impl = _int64_num_impl, attrs = {})

def bigint_test():
    _int8_num(name = "bigint_int8_num")
    _int32_num(name = "bigint_int32_num")
    _int64_num(name = "bigint_int64_num")
