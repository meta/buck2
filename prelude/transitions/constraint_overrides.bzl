# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cfg/modifier:name.bzl", "cfg_name")
# NOTE: At the moment, target platforms and constraint values can't be propagated via build rule
#       attributes all the way to configuration transitions. The reason is that the transition
#       function is invoked before the rule, so any PlatformInfo and ConstraintValueInfo providers
#       present in the rule attributes aren't resolved at this time.
#
#       Because of this, the list of possible target platforms and constraint values is encoded
#       here in the form of config settings with lists of comma-separated elements. Specifically,
#       the configuration transition relies on the following config settings:
#
#           * buck2.platforms
#           * buck2.constraints
#           * buck2.passthrough_constraints
#
#       buck2.platforms and buck2.constraints list all possible target platforms and constraint
#       values, and buck2.passthrough_constraints lists constraint settings to preserve when
#       applying the configuration transition. The use of read_config avoids hard-coding these
#       repo-specific configuration rules into the prelude.
load("@prelude//python:transitions.bzl", "python_transitions")
load("@prelude//transitions:constraint_override_registry.bzl", "ConstraintOverrideRegistryInfo", "constraint_override_refs")

_config = struct(
    passthrough_constraints = read_root_config("buck2", "passthrough_constraints", ""),
    split = lambda values: [value.strip() for value in values.split(",") if value.strip()],
)

def _passthrough_constraints() -> list[str]:
    return _config.split(_config.passthrough_constraints)

def _check(target: str) -> str:
    """
    Checks that a build target is fully qualified.

    Because of the context in which the transition function is invoked, it can't make use of
    certain functions that are only available in a build file context. These functions include
    package_name and get_cell_name. For this reason all build targets must be fully qualified, in
    the format cell//package:name, so that they may be used for look-up in refs.

    Args:
        target  Build  target.

    Returns:
        Fully qualified build target.
    """
    message = "Build target must be fully qualified, expected format cell//package:name but received: {target}".format(
        target = target,
    )
    if target.count("//") != 1:
        fail(message)
    cell, path = target.split("//")
    if len(cell) == 0:
        fail(message)
    if path.count(":") != 1:
        fail(message)
    package, name = path.split(":")
    return "{cell}//{package}:{name}".format(cell = cell, package = package, name = name)

def _resolve(refs: struct, attrs: struct) -> dict[str, PlatformInfo | list[ConstraintValueInfo] | None]:
    """
    Validates and prepares arguments for business logic in constraint_overrides.apply, which
    implements the transition function.

    All target platforms and constraint values present in attrs must also be present in refs, which
    contains the list of all possible target platforms and constraint values. If they're not, the
    configuration transition fails with an error message saying the override is not supported.

    Args:

        refs   struct containing the list of all possible target platforms and constraint values,
               stored as a mapping from build targets to configured target labels with PlatformInfo
               and ConstraintValueInfo providers.
        attrs  Struct containing the platform_override and constraint_overrides arguments to the
               build rule to which the configuration transition is being applied.

    Returns:
        dict containing key-value arguments to passy to constraint_overrides.apply.
    """
    args = {}

    # Resolve target platform override.
    override = None
    if hasattr(attrs, "platform_override") and attrs.platform_override != None:
        override = _check(attrs.platform_override)
    args["platform"] = None
    if override:
        if not hasattr(refs, override):
            fail(
                "Target platform override not supported: {override}".format(
                    override = override,
                )
            )
        ref = getattr(refs, override)
        args["platform"] = ref[PlatformInfo]

    # Resolve constraint value overrides.
    overrides = []
    if hasattr(attrs, "constraint_overrides") and attrs.constraint_overrides != None:
        overrides = [_check(override) for override in attrs.constraint_overrides if override != None]
    args["constraints"] = []
    for override in overrides:
        if not hasattr(refs, override):
            fail(
                "Constraint value override not supported: {override}".format(
                    override = override,
                )
            )
        ref = getattr(refs, override)
        args["constraints"].append(ref[ConstraintValueInfo])

    return args

def _apply(old_platform: PlatformInfo, *, platform: PlatformInfo | None = None, constraints: list[ConstraintValueInfo] = []) -> PlatformInfo:
    """
    Applies the business logic to implement the constraint_overrides configuration transition.

    The outgoing target platform is constructed by:

        1. Initializing it from old_platform, if platform is None, or from platform if not None;
        2. Applying the constraint values in old_platform whose constraint settings match any of
           those listed in buck2.passthrough_constraints; and
        3. Applying the constraint values present in constraints.

    If platform is None the constraint values in old_platform are preserved unless overridden by
    constraint values in constraints. If platform is not None the constraint values in old_platform
    are discarded unless:

        * Their constraint settings are listed in buck2.passthrough_constraints; and
        * They're not overridden by a constraint value in constraints.

    Args:

        old_platform  Incoming target platform. This is the target platform prior to the
                      configuration transition.
        platform      Target platform to use as a basis for the outgoing target platform. If None,
                      old_platform is used instead.
        constraints   List of constraint values to apply to the target platform. These constraint
                      values are applied on top of either old_platform or platform.

    Returns:
        Outgoing target platform.
    """

    # Store passthrough constraint values.
    old_constraints = {str(setting): constraint for setting, constraint in old_platform.configuration.constraints.items()}
    old_constraints = [old_constraints[setting] for setting in _passthrough_constraints() if setting in old_constraints]

    # Switch target platform.
    platform = platform or old_platform

    # Add passthrough constraint values and apply constraint value overrides.
    new_constraints = {label: constraint for label, constraint in platform.configuration.constraints.items()}
    for constraint in old_constraints:
        new_constraints[constraint.setting.label] = constraint
    for constraint in constraints:
        new_constraints[constraint.setting.label] = constraint

    new_cfg = ConfigurationInfo(
        constraints = new_constraints,
        values = platform.configuration.values,
    )

    label = platform.label
    if constraints:
        label = cfg_name(new_cfg)

    new_platform = PlatformInfo(
        label = label,
        configuration = new_cfg,
    )
    return new_platform

def _impl(platform: PlatformInfo, refs: struct, attrs: struct) -> PlatformInfo:
    if hasattr(refs, "_constraint_override_registry"):
        refs = refs._constraint_override_registry[ConstraintOverrideRegistryInfo].refs
    args = _resolve(refs, attrs)
    if args["platform"] == None and not args["constraints"]:
        return platform
    return _apply(platform, **args)

# Chain the override impl with python specific transitions.
# We do this here because the constraint_override transition relies on autogenerated refs
# Its easier to add the opt-by-default specific ones here than vice-versa
def _python_impl(platform: PlatformInfo, refs: struct, attrs: struct) -> PlatformInfo:
    # @oss-disable[end= ]: platform = python_transitions.transition_opt_by_default_impl(platform, refs, attrs)
    # @oss-disable[end= ]: platform = python_transitions.transition_omnibus_large_code_model_impl(platform, refs, attrs)
    return _impl(platform, refs, attrs)

_refs = constraint_override_refs()
# Registry dependencies enforce visibility, unlike direct transition refs.
_registry = read_root_config("buck2", "constraint_override_registry", "")
_transition_refs = {"_constraint_override_registry": _registry} if _registry else _refs
_python_refs = {k: v for k, v in _transition_refs.items()}
# @oss-disable[end= ]: _python_refs.update(python_transitions.refs())

_attributes = {
    "constraint_overrides": attrs.list(attrs.option(attrs.string()), default = []),
    "platform_override": attrs.option(attrs.string(), default = None),
}

_python_attributes = {k: v for k, v in _attributes.items()}
# @oss-disable[end= ]: _python_attributes.update(python_transitions.attrs())

_transition = transition(
    impl = _impl,
    refs = _transition_refs,
    attrs = _attributes.keys(),
)

_python_transition = transition(
    impl = _python_impl,
    refs = _python_refs,
    attrs = _python_attributes.keys(),
)

constraint_overrides = struct(
    refs = _refs,
    resolve = _resolve,
    apply = _apply,
    transition = _transition,
    attributes = _attributes,
    python_transition = _python_transition,
)
