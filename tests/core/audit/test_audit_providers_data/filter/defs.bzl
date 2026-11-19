# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

FooInfo = provider(fields = [])
BarInfo = provider(fields = [])

def _impl_both(_ctx):
    return [DefaultInfo(), FooInfo(), BarInfo()]

def _impl_foo_only(_ctx):
    return [DefaultInfo(), FooInfo()]

both_rule = rule(impl = _impl_both, attrs = {})
foo_rule = rule(impl = _impl_foo_only, attrs = {})
