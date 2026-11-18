# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//utils:type_defs.bzl", "is_select")

# One way of building the target. Labels are unconfigured on purpose: the
# variant is built in its own configuration, not the one the rule was
# analyzed in. Modifiers keep their subtarget so `cpu[arm64]` round-trips.
BuildVariant = record(
    buckconfigs = field(dict[str, str]),
    description = field(str),
    metadata = field(dict[str, str]),
    modefiles = field(list[str]),
    modifiers = field(list[ProvidersLabel]),
    target_platform = field(TargetLabel | None),
)

BuildVariantsInfo = provider(
    doc = "The named ways of building one target.",
    fields = {
        "target": provider_field(TargetLabel),
        "variants": provider_field(dict[str, BuildVariant]),
    },
)

# The per-variant keys accepted by the `build_variants()` macro. `modifiers`
# is spelled as on the command line. The rule attributes for `metadata` and
# `modifiers` are `metadata_dicts` and `modifier_lists` because buck2 reserves
# both names as internal per-target attributes.
BUILD_VARIANT_KEYS = ["buckconfigs", "description", "metadata", "modefiles", "modifiers", "target_platform"]

def _reject_selects(name: str, path: str, value: typing.Any):
    # A variant is a fixed recipe that tooling reads back unconfigured, so a
    # `select()` anywhere in it has no configuration to resolve against.
    if is_select(value):
        fail("build_variants `{}`: `{}` must not be a select()".format(name, path))
    if type(value) == type({}):
        for key, item in value.items():
            _reject_selects(name, "{}.{}".format(path, key), item)
    elif type(value) == type([]):
        for index, item in enumerate(value):
            _reject_selects(name, "{}[{}]".format(path, index), item)

# Variant names become sub-target names and output directories, so they must
# be free of separators and whitespace.
_FORBIDDEN_IN_VARIANT_NAME = [" ", "\t", "\n", "\r", ":", "/", "\\", "."]

def _check_variant_name(name: str, variant: str):
    for c in _FORBIDDEN_IN_VARIANT_NAME:
        if c in variant:
            fail("build_variants `{}`: variant name `{}` must not contain whitespace, `:`, `/`, `\\` or `.`".format(name, variant))

def build_variants_macro(build_variants_rule, name: str, target: str, variants: dict[str, dict[str, typing.Any]], **kwargs):
    """Splits the inline `variants` dict into the typed attributes of `build_variants()`."""
    if not target.startswith(":") or len(target) == 1 or "/" in target or target.count(":") != 1:
        fail("build_variants `{}`: target `{}` must be `:NAME`, a target in the same package".format(name, target))
    _reject_selects(name, "variants", variants)
    for variant, spec in variants.items():
        _check_variant_name(name, variant)
        for key in spec:
            if key not in BUILD_VARIANT_KEYS:
                fail("build_variants `{}`: variant `{}` has unknown key `{}`, expected one of {}".format(name, variant, key, BUILD_VARIANT_KEYS))
    build_variants_rule(
        name = name,
        target = target,
        variants = list(variants.keys()),
        buckconfigs = {variant: spec.get("buckconfigs", {}) for variant, spec in variants.items()},
        descriptions = {variant: spec.get("description", "") for variant, spec in variants.items()},
        metadata_dicts = {variant: spec.get("metadata", {}) for variant, spec in variants.items()},
        modefiles = {variant: spec.get("modefiles", []) for variant, spec in variants.items()},
        modifier_lists = {variant: spec.get("modifiers", []) for variant, spec in variants.items()},
        target_platforms = {variant: spec["target_platform"] for variant, spec in variants.items() if spec.get("target_platform") != None},
        **kwargs,
    )

def _has_whitespace(s: str) -> bool:
    for c in [" ", "\t", "\n", "\r"]:
        if c in s:
            return True
    return False

def _check_modefile(variant: str, modefile: str):
    cell, sep, _ = modefile[1:].partition("//")
    if not modefile.startswith("@") or not sep or not cell:
        fail("variant `{}`: modefile `{}` must start with `@cell//`".format(variant, modefile))
    if _has_whitespace(modefile):
        fail("variant `{}`: modefile `{}` must not contain whitespace".format(variant, modefile))

def _check_buckconfig(variant: str, key: str, value: str):
    section, sep, name = key.partition(".")
    if not sep or not section or not name:
        fail("variant `{}`: buckconfig key `{}` must be `section.key`".format(variant, key))
    if _has_whitespace(key) or _has_whitespace(value):
        fail("variant `{}`: buckconfig `{}={}` must not contain whitespace".format(variant, key, value))

def _unconfigured(label: ConfiguredProvidersLabel) -> ProvidersLabel:
    return label.raw_target().with_sub_target(label.sub_target or [])

def _build_variant(ctx: AnalysisContext, variant: str) -> BuildVariant:
    buckconfigs = ctx.attrs.buckconfigs.get(variant, {})
    modefiles = ctx.attrs.modefiles.get(variant, [])
    target_platform = ctx.attrs.target_platforms.get(variant)
    for modefile in modefiles:
        _check_modefile(variant, modefile)
    for key, value in buckconfigs.items():
        _check_buckconfig(variant, key, value)
    return BuildVariant(
        buckconfigs = buckconfigs,
        description = ctx.attrs.descriptions.get(variant, ""),
        metadata = ctx.attrs.metadata_dicts.get(variant, {}),
        modefiles = modefiles,
        modifiers = [_unconfigured(m.label) for m in ctx.attrs.modifier_lists.get(variant, [])],
        target_platform = target_platform.label.raw_target() if target_platform else None,
    )

def _json(variant: BuildVariant) -> dict[str, typing.Any]:
    return {
        "buckconfigs": variant.buckconfigs,
        "description": variant.description,
        "metadata": variant.metadata,
        "modefiles": variant.modefiles,
        "modifiers": [str(m) for m in variant.modifiers],
        "target_platform": str(variant.target_platform) if variant.target_platform else None,
    }

def _cli_args(variant: BuildVariant) -> str:
    args = list(variant.modefiles)
    for modifier in variant.modifiers:
        args += ["--modifier", str(modifier)]
    for key, value in variant.buckconfigs.items():
        args += ["--config", "{}={}".format(key, value)]
    if variant.target_platform:
        args += ["--target-platforms", str(variant.target_platform)]
    return " ".join(args)

def _build_variants_impl(ctx: AnalysisContext) -> list[Provider]:
    variants = {name: _build_variant(ctx, name) for name in ctx.attrs.variants}

    sub_targets = {}
    for name, variant in variants.items():
        spec_json = ctx.actions.write_json("{}/spec.json".format(name), _json(variant), pretty = True)
        cli_args = ctx.actions.write("{}/cli-args.txt".format(name), _cli_args(variant) + "\n")
        sub_targets[name] = [
            DefaultInfo(
                default_output = spec_json,
                sub_targets = {"cli-args": [DefaultInfo(default_output = cli_args)]},
            )
        ]

    all_json = ctx.actions.write_json(
        "variants.json",
        {
            "target": str(ctx.attrs.target.raw_target()),
            "variants": {name: _json(variant) for name, variant in variants.items()},
        },
        pretty = True,
    )
    return [
        DefaultInfo(default_output = all_json, sub_targets = sub_targets),
        BuildVariantsInfo(target = ctx.attrs.target.raw_target(), variants = variants),
    ]

build_variants = prelude_rule(
    name = "build_variants",
    impl = _build_variants_impl,
    docs = """
        A `build_variants()` rule lists the named ways of building one target: for each variant, the
        target platform to build for, and the buckconfigs, modefiles and modifiers to build with. A
        variant may also carry `metadata`, string key-value pairs for tooling to read, which do not
        change how the variant is built.

        The `target` must be in the same package, written `:NAME`, so that a target's variants are
        always found next to it. The variants are given inline as a dict from variant name to a dict
        with any of the keys `description`, `buckconfigs`, `metadata`, `modefiles`, `modifiers` and
        `target_platform`; the macro layer splits that into the per-key attributes below. A variant
        is a fixed recipe, so no part of it may be a `select()`. Variant names are used as sub-target
        names, so they may not contain whitespace, `:`, `/`, `\\` or `.`.

        Its default output is a JSON file with the target and every variant. The `[VARIANT]`
        sub-target is the JSON of one variant, and `[VARIANT][cli-args]` is a single line of
        `buck2 build` arguments for it: modefiles, then modifiers, then buckconfigs, then the target
        platform.
    """,
    examples = """
        ```

        build_variants(
          name = 'DemoApp-build-variants',
          target = ':DemoApp',
          variants = {
            'release': {
              'description': 'Release build, as shipped to the App Store',
              'target_platform': 'config//platform/ios:arm64-release',
              'buckconfigs': {'apple.codesign_type': 'distribution'},
              'metadata': {'distribution_channel': 'app_store'},
              'modefiles': ['@fbsource//fbobjc/mode/release'],
              'modifiers': ['config//toolchain/constraints:xcode-pika'],
            },
          },
        )

        ```
    """,
    further = None,
    # The variant data is actually unwrapped, so that we can encode validation
    # using the attrs. For example:
    #
    # variants = {
    #   'release': {
    #     'buckconfigs': {'apple.codesign_type': 'distribution'},
    #   },
    # },
    #
    # would end up as having `'release': {'apple.codesign_type': 'distribution'}`
    # for the `buckconfigs` attribute.
    attrs = (
        {
            "buckconfigs": attrs.dict(
                key = attrs.string(),
                value = attrs.dict(key = attrs.string(), value = attrs.string(), sorted = True),
                sorted = False,
                default = {},
                doc = """
                Per variant, buckconfig overrides keyed by `section.key`. Neither key nor value may
                contain whitespace.
            """,
            ),
            "descriptions": attrs.dict(
                key = attrs.string(),
                value = attrs.string(),
                sorted = False,
                default = {},
                doc = """
                Per variant, a free-form, human-readable description for tooling to display.
            """,
            ),
            # Not `metadata`: buck2 reserves that name for its internal per-target
            # attribute and rejects rules that declare it.
            "metadata_dicts": attrs.dict(
                key = attrs.string(),
                value = attrs.dict(key = attrs.string(), value = attrs.string(), sorted = True),
                sorted = False,
                default = {},
                doc = """
                Per variant, string key-value pairs for tooling to read. They do not change how the
                variant is built.
            """,
            ),
            "modefiles": attrs.dict(
                key = attrs.string(),
                value = attrs.list(attrs.string()),
                sorted = False,
                default = {},
                doc = """
                Per variant, the modefiles in the order they are passed on the command line. Each must
                be cell-qualified, `@cell//path/to/mode`, and must not contain whitespace.
            """,
            ),
            # Not `modifiers`: buck2 reserves that name for its internal per-target
            # attribute and rejects rules that declare it.
            "modifier_lists": attrs.dict(
                key = attrs.string(),
                value = attrs.list(attrs.configuration_label()),
                sorted = False,
                default = {},
                doc = """
                Per variant, the constraint values passed with `--modifier`.
            """,
            ),
            "target": attrs.label(
                doc = """
                The target the variants build. It is not a dependency of this rule and is not
                configured by it.
            """,
            ),
            "target_platforms": attrs.dict(
                key = attrs.string(),
                value = attrs.configuration_label(),
                sorted = False,
                default = {},
                doc = """
                Per variant, the target platform to build for. Variants without one are omitted.
            """,
            ),
            "variants": attrs.list(
                attrs.string(),
                doc = """
                The variant names, in declaration order. The per-key attributes are keyed by these.
            """,
            ),
        }
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
