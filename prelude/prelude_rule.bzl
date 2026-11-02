# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")

# The declaration of a rule that the prelude exposes via `native` and documents.
#
# Conventions:
#  - A rule is declared in the `.bzl` file that implements it, next to its
#    implementation function.
#  - Attribute groups shared between the rules of one language live in that
#    language's `attrs.bzl`; groups shared across languages live in `decls/`.
#    Keep `attrs.bzl` files free of implementation code so that implementation
#    files can load them without cycles.
#  - Every language directory has a `rules.bzl` that turns its declarations
#    into `rule` objects with `make_rules` and exports both: the declarations
#    as `specs` and the rule objects as `rules`. `prelude//:rules.bzl` only
#    collects those. The rule objects are created in the per-language files on
#    purpose: a target's rule type is attributed to the file that creates the
#    `rule`, so this keeps the set of files whose changes make CI rebuild a
#    target down to that target's own language.
prelude_rule = record(
    name = field(str),
    docs = field([str, None], None),
    examples = field([str, None], None),
    further = field([str, None], None),
    attrs = field(dict[str, Attr]),
    impl = field([typing.Callable, None], None),
    uses_plugins = field([list[plugins.PluginKind], None], None),
    exec_groups = field([list[ExecGroup], None], None),
    supports_incoming_transition = field([bool, None], None),
    is_toolchain_rule = field(bool, False),
    is_configuration_rule = field(bool, False),
    # TODO(mgd): should be `transition | None`, but `transition` does not work as type.
    cfg = field(typing.Any | None, None),
)

def _unimplemented(name, ctx):
    fail("Unimplemented rule type `{}` for target `{}`.".format(name, ctx.label))

def _unimplemented_impl(name):
    # We could use a lambda here, but then it means every single parse evaluates a lambda.
    # Lambda's have tricky semantics, so using partial lets us test Starlark prototypes with
    # some features disabled.
    return partial(_unimplemented, name)

def make_rule(spec: prelude_rule, extra_attrs: dict[str, typing.Any] = dict(), impl_override: [typing.Callable, None] = None, **kwargs):
    """The `rule` object for a declaration, with the prelude-wide policies applied."""
    name = spec.name
    attributes = spec.attrs

    # We want native code-containing rules to be marked incompatible with fat
    # platforms. Getting the ones that use cxx/apple toolchains is a little
    # overly broad as it includes things like python that don't themselves have
    # native code but need the toolchains if they depend on native code and in
    # that case incompatibility is transitive and they'll get it.
    fat_platform_compatible = True
    if name not in ("python_library", "python_binary", "python_test"):
        for toolchain_attr in ("_apple_toolchain", "_cxx_toolchain", "_go_toolchain"):
            if toolchain_attr in attributes:
                fat_platform_compatible = False

    # Fat platforms is an idea specific to our toolchains, so doesn't apply to
    # open source. Ideally this restriction would be done at the toolchain level.
    if not is_full_meta_repo():
        fat_platform_compatible = True

    attributes = dict(attributes)
    attributes.update(extra_attrs)
    if not fat_platform_compatible:
        # copy so we don't try change the passed in object
        attributes["_cxx_toolchain_target_configuration"] = attrs.dep(default = "prelude//platforms:fat_platform_incompatible")

    cfg = spec.cfg

    if cfg != None:
        # Allow clone_rule callers to set their own cfg, if it exists
        kwargs.setdefault("cfg", cfg)

    if spec.docs:
        doc = spec.docs

        # This is awkward. When generating documentation, we'll strip leading whitespace
        # like it's a python docstring. For that to work here, we need the "Examples:" line
        # to match the other lines for leading whitespace. We've just hardcoded this to
        # be what its expected to be in prelude.
        # TODO(cjhopman): Figure out something better here.
        if spec.examples:
            # NOTE: DocString needs 'Examples:' to render the sections properly, that code should be updated along with this
            doc += "\n{}Examples:\n{}".format(" " * 8, spec.examples)
        if spec.further:
            doc += "\n{}Additional notes:\n{}".format(" " * 8, spec.further)

        kwargs["doc"] = doc

    impl = spec.impl
    if not impl:
        impl = _unimplemented_impl(name)
    if impl_override != None:
        impl = impl_override
    if spec.uses_plugins != None:
        kwargs["uses_plugins"] = spec.uses_plugins
    if spec.exec_groups != None:
        kwargs["exec_groups"] = spec.exec_groups
    if spec.supports_incoming_transition != None:
        kwargs["supports_incoming_transition"] = spec.supports_incoming_transition

    kwargs.setdefault("is_configuration_rule", spec.is_configuration_rule)
    kwargs.setdefault("is_toolchain_rule", spec.is_toolchain_rule)
    return rule(impl = impl, attrs = attributes, **kwargs)

def make_rules(specs: dict[str, prelude_rule]) -> dict[str, typing.Any]:
    """The `rule` objects for a registry's declarations, keyed by rule name; bind them with `load_symbols`."""
    for name, spec in specs.items():
        if spec.name != name:
            fail("Rule `{}` is registered under the name `{}`".format(spec.name, name))
    return {name: make_rule(spec) for name, spec in specs.items()}
