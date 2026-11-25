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
    docs = """
        An `alias()` gives another name to a target. Building, running, testing or depending on the
        alias has the same effect as doing so with the target that `actual` names: the alias has the
        outputs, the subtargets and all the other providers of that target. For a toolchain, use
        `toolchain_alias()`.
    """,
    examples = """
        ```
        # //:fmt is another name for //tools/format:fmt.
        alias(
            name = "fmt",
            actual = "//tools/format:fmt",
            visibility = ["PUBLIC"],
        )
        ```

        `buck2 run //:fmt` then runs `//tools/format:fmt`.
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "actual": attrs.option(
                attrs.dep(pulls_and_pushes_plugins = plugins.All),
                doc = """
                The target that this alias stands for.
            """,
            ),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)

configured_alias = prelude_rule(
    name = "configured_alias",
    impl = configured_alias_impl,
    docs = """
        A `configured_alias()` is an alias for a target as it is built for one platform. The target
        that `actual` names is configured for `platform`, whatever the configuration of the alias and
        of the targets that depend on it, and the alias has the outputs and all the other providers
        of that configured target.

        In a BUCK file, `configured_alias()` takes `actual` and `platform`, and sets
        `configured_actual` and `fallback_actual` from them. It also takes
        `fallback_to_unconfigured_alias`: when that is `True` and `platform` is `None`, which a
        `select()` can produce, `actual` is built in the configuration of the alias, as with
        `alias()`.
    """,
    examples = """
        ```
        # //:server-arm64 is //:server built for the platform //platforms:linux-arm64,
        # whichever platform the build is for.
        configured_alias(
            name = "server-arm64",
            actual = ":server",
            platform = "//platforms:linux-arm64",
        )
        ```
    """,
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
            "actual": attrs.label(
                doc = """
                The target that this alias stands for.
            """,
            ),
            "configured_actual": attrs.option(
                attrs.configured_dep(),
                default = None,
                doc = """
                The target to build, paired with the platform to build it for. `configured_alias()` sets
                this from `actual` and `platform`: it is an error to pass it in a BUCK file.
            """,
            ),
            "fallback_actual": attrs.option(
                attrs.dep(),
                default = None,
                doc = """
                The target to build in the configuration of the alias. `configured_alias()` sets this to
                `actual`, in place of `configured_actual`, when `platform` is `None` and
                `fallback_to_unconfigured_alias` is `True`: it is an error to pass it in a BUCK file.
            """,
            ),
            # We use a separate field instead of re-purposing `actual`, as we want
            # to keep output format compatibility with v1.
            # If `configured_actual` is `None`, fallback to this unconfigured dep.
            "platform": attrs.option(
                attrs.configuration_label(),
                default = None,
                doc = """
                The platform to build `actual` for, as the label of a `platform()` target. A BUCK file has
                to pass it, and it can be `None` only with `fallback_to_unconfigured_alias = True`.
            """,
            ),
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
    examples = """
        ```
        load("@prelude//toolchains:cxx.bzl", "system_cxx_toolchain")

        system_cxx_toolchain(
            name = "cxx_gcc",
            compiler = "gcc",
            cxx_compiler = "g++",
            linker = "g++",
            compiler_type = "gcc",
            visibility = ["PUBLIC"],
        )

        # In the root BUCK file of the `toolchains` cell: the rules that take their C++
        # toolchain from `toolchains//:cxx` get `:cxx_gcc`.
        toolchain_alias(
            name = "cxx",
            actual = ":cxx_gcc",
            visibility = ["PUBLIC"],
        )
        ```
    """,
    further = None,
    attrs = {
        "actual": attrs.option(
            attrs.toolchain_dep(),
            doc = """
            The toolchain target that this alias stands for. It must be a toolchain rule.
        """,
        ),
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
