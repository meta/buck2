# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:argfile.bzl", "at_argfile")

def _worker_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        WorkerInfo(
            exe = ctx.attrs.worker[RunInfo].args,
            concurrency = None,
            supports_bazel_remote_persistent_worker_protocol = True,
        ),
    ]

worker = rule(
    impl = _worker_impl,
    attrs = {
        "worker": attrs.dep(providers = [RunInfo]),
    },
)

def _demo_impl(ctx: AnalysisContext) -> list[Provider]:
    output = ctx.actions.declare_output(ctx.label.name, has_content_based_path = False)
    argfile = at_argfile(
        actions = ctx.actions,
        name = "demo." + ctx.label.name + ".args",
        args = cmd_args(output.as_output()),
    )
    ctx.actions.run(
        cmd_args(argfile),
        category = "demo",
        env = {
            # modify this value to force an action rerun even if caching is enabled.
            # `--no-remote-cache` does not have the desired effect, because it also causes
            # the action to be omitted from `buck2 log what-ran`, which interferes with the
            # test setup.
            "CACHE_SILO_KEY": read_root_config("build", "cache_silo_key", "0"),
        },
        exe = WorkerRunInfo(
            worker = ctx.attrs._worker[WorkerInfo],
            exe = ctx.attrs._one_shot[RunInfo].args,
        ),
    )
    return [DefaultInfo(default_output = output)]

demo = rule(
    impl = _demo_impl,
    attrs = {
        "_one_shot": attrs.exec_dep(
            default = "//:one_shot",
            providers = [RunInfo],
        ),
        "_worker": attrs.exec_dep(
            default = "//:worker",
            providers = [WorkerInfo],
        ),
    },
)
