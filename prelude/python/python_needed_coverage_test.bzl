# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:test_common.bzl", "test_common")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")
load(
    "@prelude//tests:re_utils.bzl",
    "get_re_executors_from_props",
)
load(
    ":needed_coverage.bzl",
    "parse_python_needed_coverage_specs",
)

def python_needed_coverage_test_impl(ctx: AnalysisContext) -> list[Provider]:
    test_cmd = list(ctx.attrs.test[ExternalRunnerTestInfo].command)

    test_env = {}
    test_env.update(ctx.attrs.env)

    # Pass in needed coverage flags to the test.
    needed_coverages = parse_python_needed_coverage_specs(ctx.attrs.needed_coverage)
    test_cmd.append("--collect-coverage")
    test_cmd.append("--coverage-include")
    test_cmd.append(",".join(["*/{}".format(module) for needed_coverage in needed_coverages for module in needed_coverage.modules]))
    for needed_coverage in needed_coverages:
        for module in needed_coverage.modules:
            test_cmd.append("--coverage-verdict={}={}".format(module, needed_coverage.ratio))

    # A needed coverage run just runs the entire test binary as bundle to
    # determine coverage.  Rather than special implementation in tpx, we
    # just use a simple test type to do this, which requires settings a few
    # additional flags/env-vars which the Python tpx test type would
    # otherwise handle.
    test_type = "simple"
    test_env["TEST_PILOT"] = "1"

    # Setup RE executors based on the `remote_execution` param.
    re_executors = get_re_executors_from_props(ctx)

    return inject_test_run_info(
        ctx,
        ExternalRunnerTestInfo(
            type = test_type,
            command = test_cmd,
            env = test_env,
            labels = ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            default_executor = re_executors.default_executor,
            executor_overrides = re_executors.executor_overrides,
            # We implicitly make this test via the project root, instead of
            # the cell root (e.g. fbcode root).
            run_from_project_root = re_executors.run_from_project_root,
            use_project_relative_paths = re_executors.use_project_relative_paths,
            supports_test_execution_caching = ctx.attrs.supports_test_execution_caching,
        ),
    ) + [
        DefaultInfo(),
    ]

python_needed_coverage_test = prelude_rule(
    name = "python_needed_coverage_test",
    impl = python_needed_coverage_test_impl,
    docs = """
        A `python_needed_coverage_test()` rule wraps an existing
        `python_test()` target so that it runs as a coverage-collecting
        bundle and is checked against per-module coverage thresholds. It is
        used to enforce that specific modules covered by the wrapped test
        meet a minimum coverage ratio at test time.
    """,
    examples = None,
    further = None,
    attrs = dict(
        contacts = attrs.list(attrs.string(), default = []),
        env = attrs.dict(key = attrs.string(), value = attrs.arg(), sorted = False, default = {}),
        labels = attrs.list(attrs.string(), default = []),
        needed_coverage = attrs.list(attrs.tuple(attrs.int(), attrs.dep(), attrs.option(attrs.string())), default = []),
        network_access = test_common.attributes()["network_access"],
        supports_test_execution_caching = attrs.bool(default = True),
        test = attrs.dep(providers = [ExternalRunnerTestInfo]),
        **(re_test_common.test_args() | buck.inject_test_env_arg()),
    ),
)
