# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @nolint

# Number of artifacts re-bound through the dynamic_output. The cost of the
# regression this fixture guards against is quadratic in this (each re-bound
# artifact's action key holds the producing action's full output map as its
# dice value, and each gets equality-compared on rebuild), while all the
# legitimate work is linear. At 50k the quadratic cost is minutes of serialized
# CPU while the initial build stays under a minute.
N = 50000

# Long path segments make each map probe (hash plus compare of a
# `BuildArtifactPath`) more expensive, amplifying the quadratic cost without
# growing N.
PAD = "x" * 180

def _impl(ctx: AnalysisContext) -> list[Provider]:
    outs = [
        ctx.actions.declare_output("outs/{}/{}/{}.txt".format(PAD, PAD, i), has_content_based_path = False)
        for i in range(N)
    ]
    trigger = ctx.actions.write("trigger", "", has_content_based_path = False)

    def _dyn(ctx: AnalysisContext, _artifacts, outputs, outs = outs):
        # The seed is written into the *last* output only. For the comparisons
        # to be expensive the producing action's output content must change
        # between builds (an unchanged value is reused by pointer and compares
        # O(1)), and the change must be late in insertion order (map equality
        # scans until the first mismatch). This mirrors dist-ThinLTO, where the
        # handful of link-level outputs that change on a flag flip are declared
        # after all the per-object outputs.
        script = ctx.actions.write(
            "write_outs.py",
            [
                "import sys",
                "with open(sys.argv[1]) as f:",
                "    paths = f.read().splitlines()",
                "for p in paths[:-1]:",
                "    with open(p, 'w') as f:",
                "        f.write('x')",
                "with open(paths[-1], 'w') as f:",
                "    f.write(sys.argv[2])",
            ],
            has_content_based_path = False,
        )
        out_args = [outputs[o].as_output() for o in outs]
        argsfile, _ = ctx.actions.write("outs_list.txt", out_args, allow_args = True, has_content_based_path = False)
        # A single action binding all of the re-bound outputs, mirroring the
        # dist-ThinLTO index action.
        ctx.actions.run(
            cmd_args("python3", script, argsfile, ctx.attrs.seed, hidden = out_args),
            category = "big_write",
        )

    ctx.actions.dynamic_output(
        dynamic = [trigger],
        inputs = [],
        outputs = [o.as_output() for o in outs],
        f = _dyn,
    )

    # Consume every artifact individually so that each one's re-bound action
    # key actually gets built.
    stamp = ctx.actions.symlinked_dir("stamp", {str(i): o for i, o in enumerate(outs)})
    return [DefaultInfo(default_output = stamp)]

many_rebound_outputs = rule(
    impl = _impl,
    attrs = {
        "seed": attrs.string(),
    },
)
