# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

ConstraintOverrideRegistryInfo = provider(
    fields = {"refs": provider_field(struct)},
)

def constraint_override_refs() -> dict[str, str]:
    overrides = read_root_config("buck2", "platforms", "") + "," + read_root_config("buck2", "constraints", "")
    return {override.strip(): override.strip() for override in overrides.split(",") if override.strip()}

def _constraint_override_registry_impl(ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(), ConstraintOverrideRegistryInfo(refs = struct(**ctx.attrs.refs))]

constraint_override_registry = rule(
    impl = _constraint_override_registry_impl,
    attrs = {"refs": attrs.dict(attrs.string(), attrs.dep())},
    is_configuration_rule = True,
)
