# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//java:compiler_depth.bzl", "compiler_depth_report", "max_compiler_depth")
load("@prelude//java:java_providers.bzl", "JavaPackagingDep")

def compiler_depth_subtarget(ctx: AnalysisContext, packaging_deps: list[JavaPackagingDep]) -> list[Provider]:
    limit = getattr(ctx.attrs, "max_jvm_compiler_depth", None)
    if limit != None and limit < 0:
        fail("max_jvm_compiler_depth must be non-negative, got {}".format(limit))
    deepest = max_compiler_depth([dep.compiler_depth for dep in packaging_deps])
    report = compiler_depth_report(deepest)
    if limit != None and report["depth"] > limit:
        fail(
            """{} has JVM compiler depth {}, exceeding max_jvm_compiler_depth = {}.
Compiler chain (dependencies first):
{}

To reduce compiler depth, shorten a dependency path in the chain above:
  - Cut unnecessary dependencies between targets in the chain (deps, exported_deps, provided_deps), or split a target to depend on a smaller API.
  - Enable abi_generation_mode = "source_only" on a library in the chain so consumers need not wait for its full library compilation.
  - For a source-only ABI step, refactor code to remove unnecessary source_only_abi_deps entries, or set required_for_source_only_abi = False on a dependency if it is no longer needed for ABI generation.
Rebuild affected targets to validate ABI changes, then rerun analysis; another chain may become the longest.""".format(
                ctx.label,
                report["depth"],
                limit,
                "\n".join(["  {}: {} ({})".format(i + 1, step["target"], step["category"]) for i, step in enumerate(report["chain"])]),
            )
        )
    report["max_jvm_compiler_depth"] = limit
    return [DefaultInfo(default_output = ctx.actions.write_json("jvm_compiler_depth.json", report))]
