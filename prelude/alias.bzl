# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
# Implementation of aliases build rules.

def alias_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs.actual:
        return ctx.attrs.actual.providers
    else:
        return [DefaultInfo()]

def configured_alias_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs.configured_actual != None and ctx.attrs.fallback_actual != None:
        fail("cannot set both of `configured_actual` and `fallback_actual`")
    if ctx.attrs.configured_actual != None:
        return ctx.attrs.configured_actual.providers
    if ctx.attrs.fallback_actual != None:
        return ctx.attrs.fallback_actual.providers
    fail("must set one of `configured_actual` or `fallback_actual`")

def versioned_alias_impl(_ctx: AnalysisContext) -> list[Provider]:
    # Should be intercepted in macro stub and converted to `alias`.
    fail("unsupported")

alias = prelude_rule(
    name = "alias",
    impl = alias_impl,
    docs = "",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "actual": attrs.option(attrs.dep(pulls_and_pushes_plugins = plugins.All)),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)

configured_alias = prelude_rule(
    name = "configured_alias",
    impl = configured_alias_impl,
    docs = "",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            # The 'actual' attribute of configured_alias is a configured_label, which is
            # currently unimplemented. Map it to dep so we can simply forward the providers.
            # TODO(nga): "actual" attribute exists here only to display it in query,
            #   actual `actual` attribute used in rule implementation is named `configured_actual`.
            #   Logically this should be `attrs.configuration_label`, but `configuration_label`
            #   is currently an alias for `attrs.dep`, which makes non-transitioned dependency
            #   also a dependency along with transitioned dependency. (See D40255132).
            "actual": attrs.label(),
            "configured_actual": attrs.option(attrs.configured_dep(), default = None),
            "fallback_actual": attrs.option(attrs.dep(), default = None),
            # We use a separate field instead of re-purposing `actual`, as we want
            # to keep output format compatibility with v1.
            # If `configured_actual` is `None`, fallback to this unconfigured dep.
            "platform": attrs.option(attrs.configuration_label(), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)

toolchain_alias = prelude_rule(
    name = "toolchain_alias",
    impl = alias_impl,
    is_toolchain_rule = True,
    docs = """
toolchain_alias acts like alias but for toolchain rules.

The toolchain_alias itself is a toolchain rule and the `actual` argument is
expected to be a toolchain_rule as well.
    """,
    examples = None,
    further = None,
    attrs = {
        "actual": attrs.option(attrs.toolchain_dep(doc = "The actual toolchain that is being aliased. This should be a toolchain rule.")),
    },
)

versioned_alias = prelude_rule(
    name = "versioned_alias",
    impl = versioned_alias_impl,
    docs = "",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "versions": attrs.dict(key = attrs.string(), value = attrs.dep(), sorted = False, default = {}),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
