# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _fail_infra_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            "-c",
            "import sys; print('error: Transport endpoint is not connected', file=sys.stderr); sys.exit(1)",
            out.as_output(),
        ],
        category = "fail_infra",
    )
    return [DefaultInfo(default_outputs = [out])]

fail_infra = rule(impl = _fail_infra_impl, attrs = {})
