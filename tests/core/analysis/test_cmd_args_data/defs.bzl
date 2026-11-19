# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _output_artifact_in_relative_to(ctx):
    src = ctx.attrs.source
    output = ctx.actions.declare_output("myout", has_content_based_path = False)

    c = cmd_args(
        "fbpython",
        "-c",
        cmd_args(
            "import sys",
            "import os",
            "p1 = sys.argv[1]",
            "p2 = os.path.join(sys.argv[2], sys.argv[3])",
            "assert os.path.realpath(p1) == os.path.realpath(p2)",
            "open(sys.argv[2], 'w')",
            delimiter = "; ",
        ),
        src,
        output.as_output(),
        cmd_args(src, relative_to = output.as_output()),
    )

    ctx.actions.run(
        c,
        category = "run",
    )

    return [DefaultInfo(default_output = output)]

output_artifact_in_relative_to = rule(
    impl = _output_artifact_in_relative_to,
    attrs = {
        "source": attrs.source(),
    },
)
