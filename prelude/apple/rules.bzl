# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load("@prelude//apple:apple_rules_decls.bzl", "apple_rules")
# @oss-disable[end= ]: load("@prelude//apple/meta_only:meta_only_rules.bzl", "meta_only_apple_rules")

_specs = {name: getattr(apple_rules, name) for name in dir(apple_rules)}

# @oss-disable[end= ]: specs = _specs | meta_only_apple_rules()
specs = _specs # @oss-enable

rules = make_rules(specs)

load_symbols(rules)
