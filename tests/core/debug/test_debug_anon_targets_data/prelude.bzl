# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

LeafInfo = provider(fields = ["data"])

def _anon_leaf_impl(ctx):
    return [DefaultInfo(), LeafInfo(data = ctx.attrs.suffix)]

_anon_leaf = rule(impl = _anon_leaf_impl, attrs = {"suffix": attrs.string()})

def _anon_mid_impl(ctx):
    # Requests another anon target from inside an anon target.
    def process(providers):
        return [DefaultInfo(), LeafInfo(data = providers[LeafInfo].data)]

    return ctx.actions.anon_target(_anon_leaf, {"suffix": "from-mid"}).promise.map(process)

_anon_mid = rule(impl = _anon_mid_impl, attrs = {"level": attrs.int()})

def _use_anon_impl(ctx):
    def process(_providers):
        out = ctx.actions.write("out.txt", "", has_content_based_path = False)
        return [DefaultInfo(default_output = out)]

    return ctx.actions.anon_target(_anon_mid, {"level": 1}).promise.map(process)

use_anon = rule(impl = _use_anon_impl, attrs = {})

def _plain_impl(ctx):
    out = ctx.actions.write("plain.txt", "", has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

plain = rule(impl = _plain_impl, attrs = {"deps": attrs.list(attrs.dep(), default = [])})
