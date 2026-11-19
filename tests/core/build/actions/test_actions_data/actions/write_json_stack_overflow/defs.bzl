# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Deep enough to exhaust the daemon's native stack on all platforms and build
# modes if recursed over without a depth check.
_DEPTH = 100000

def _deep_value():
    x = []
    for _ in range(_DEPTH):
        x = [x]
    return x

def _deep_write_json_impl(ctx):
    out = ctx.actions.write_json("out.json", _deep_value(), has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

_deep_write_json = rule(impl = _deep_write_json_impl, attrs = {})

def _project(v):
    return v

_DeepSet = transitive_set(json_projections = {"proj": _project})

def _deep_tset_impl(ctx):
    ctx.actions.tset(_DeepSet, value = _deep_value())
    out = ctx.actions.write("out.txt", "", has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

_deep_tset = rule(impl = _deep_tset_impl, attrs = {})

def test():
    _deep_write_json(name = "deep_write_json")
    _deep_tset(name = "deep_tset")
