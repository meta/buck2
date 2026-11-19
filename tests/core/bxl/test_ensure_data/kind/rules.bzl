# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _rule_impl(_ctx):
    return [DefaultInfo()]

rule1 = rule(impl = _rule_impl, attrs = {"foo": attrs.string()})
rule2 = rule(impl = _rule_impl, attrs = {"foo": attrs.string()})
rule3 = rule(impl = _rule_impl, attrs = {"foo": attrs.string()})

def _rule_impl_with_run_info_and_default_info_outputs(ctx):
    out = ctx.actions.write("default_out", "default_out", has_content_based_path = False)
    run_info_out = ctx.actions.write("run_info_out", "run_info_out", has_content_based_path = False)
    return [
        DefaultInfo(default_outputs = [out]),
        RunInfo(args = cmd_args(run_info_out)),
    ]

rule4 = rule(
    impl = _rule_impl_with_run_info_and_default_info_outputs,
    attrs = {"foo": attrs.string()},
)

def project(f: Artifact):
    return f

NameSet = transitive_set(
    args_projections = {
        "project": project,
    }
)

NameInfo = provider(fields = ["tset"])

def _rule_impl_with_tset(ctx):
    # Produce a file that contains our name.
    out = ctx.actions.write("out.txt", str(ctx.label.name) + "\n", has_content_based_path = False)

    # Produce a tset that is our file concated wiht all the files emitted by
    # our children.
    children = [d[NameInfo].tset for d in ctx.attrs.deps]
    tset = ctx.actions.tset(NameSet, value = out, children = children)

    # Concatenate all the files declared by the tset, into a single file
    # (agg.txt), which we'll return as our output.
    agg = ctx.actions.declare_output("tset_out", has_content_based_path = False)
    projected = tset.project_as_args("project")

    ctx.actions.run(
        [
            "sh",
            "-c",
            'out="$1" && shift && cat "$@" > "$out"',
            "--",
            agg.as_output(),
            projected,
        ],
        category = "test",
    )

    return [
        NameInfo(tset = tset),
        DefaultInfo(default_output = agg),
        RunInfo(args = [projected]),
    ]

rule_tset = rule(
    impl = _rule_impl_with_tset,
    attrs = {
        "deps": attrs.list(attrs.dep(providers = [NameInfo]), default = []),
    },
)
