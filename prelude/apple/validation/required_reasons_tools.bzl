# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Tools used to validate Apple Required Reasons API (RRAPI) usage against the
# declared `.xcprivacy` privacy manifest. Whether they run is decided by the
# consuming rule.
RequiredReasonsToolsInfo = provider(
    fields = {
        # Consumes a target's own debug artifacts and emits a per-target
        # analysis JSON of the RRAPI symbols it references.
        "analyzer": provider_field(RunInfo),
        # Consumes the per-target analysis JSONs surviving in a bundle and
        # emits a single ValidationSpec result, applying the allowlist /
        # exemption checks.
        "validator": provider_field(RunInfo),
    },
)

def required_reasons_tools_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        RequiredReasonsToolsInfo(
            analyzer = ctx.attrs.analyzer[RunInfo],
            validator = ctx.attrs.validator[RunInfo],
        ),
    ]
