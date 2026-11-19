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
load("@prelude//tests:re_utils.bzl", "get_re_executors_from_props")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")

def sh_test_impl(ctx: AnalysisContext) -> list[Provider]:
    # This does not exist in v1 either, but v1 has those attributes presumably
    # to be compatible with this. We just fail if they're passed.
    if ctx.attrs.list_args or ctx.attrs.list_env or ctx.attrs.run_args or ctx.attrs.run_env:
        fail("An unsupported attribute was passed")

    args_args = []
    args_hidden = []

    if ctx.attrs.test != None:
        if isinstance(ctx.attrs.test, Artifact):
            args_args.append(ctx.attrs.test)
        elif isinstance(ctx.attrs.test, Dependency):
            run_info = ctx.attrs.test.get(RunInfo)
            if run_info != None:
                args_args.append(run_info.args)
            else:
                info = ctx.attrs.test[DefaultInfo]
                args_args.append(info.default_outputs)
                args_hidden.append(info.other_outputs)
        else:
            fail("Unexpected type for test attribute")

    args_hidden.append(ctx.attrs.resources)

    deps = []
    for dep in ctx.attrs.deps:
        info = dep[DefaultInfo]
        deps.extend(info.default_outputs)
        deps.extend(info.other_outputs)

    args_hidden.append(deps)

    args = cmd_args(args_args, hidden = args_hidden)

    command = [args] + ctx.attrs.args

    # Setup a RE executor based on the `remote_execution` param.
    re_executors = get_re_executors_from_props(ctx)

    # We implicitly make the target run from the project root if remote
    # execution options were specified
    run_from_project_root = "buck2_run_from_project_root" in (ctx.attrs.labels or []) or re_executors.run_from_project_root
    use_project_relative_paths = "buck2_run_from_project_root" in (ctx.attrs.labels or []) or re_executors.use_project_relative_paths

    # TODO support default info and runinfo properly by writing a sh script that invokes the command properly

    return inject_test_run_info(
        ctx,
        ExternalRunnerTestInfo(
            type = ctx.attrs.type or "custom",
            command = command,
            env = ctx.attrs.env,
            labels = ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            default_executor = re_executors.default_executor,
            executor_overrides = re_executors.executor_overrides,
            run_from_project_root = run_from_project_root,
            use_project_relative_paths = use_project_relative_paths,
            supports_test_execution_caching = ctx.attrs.supports_test_execution_caching,
        ),
    ) + [
        DefaultInfo(),
    ]

sh_test = prelude_rule(
    name = "sh_test",
    impl = sh_test_impl,
    docs = """
        A `sh_test()` is a test rule that can pass results to the test runner by invoking a shell script.

        **NOTE:** This rule is not currently supported on Windows.
    """,
    examples = """
        This sh\\_test() fails if a string does not match a value.

        ```
        # $REPO/BUCK
        sh_test(
            name = "script_pass",
            test = "script.sh",
            args = ["--pass"],
        )

        sh_test(
            name = "script_fail",
            test = "script.sh",
            args = ["--fail"],
        )
        ```

        ```
        # Create a simple script that prints out the resource
        $ cat > script.sh
        #!/bin/sh
        for arg in $@; do
          if [ "$arg" == "--pass" ]; then
            echo "Passed"
            exit 0;
          fi
        done
        echo "Failed"
        exit 1

        # Make sure the script is executable
        $ chmod u+x script.sh

        # Run the script, and see that one test passes, one fails
        $ buck test //:script_pass //:script_fail
        FAILURE script.sh sh_test
        Building: finished in 0.0 sec (100%) 2/2 jobs, 0 updated
          Total time: 0.0 sec
        Testing: finished in 0.0 sec (1 PASS/1 FAIL)
        RESULTS FOR //:script_fail //:script_pass
        FAIL    <100ms  0 Passed   0 Skipped   1 Failed   //:script_fail
        FAILURE script.sh sh_test
        ====STANDARD OUT====
        Failed

        PASS    <100ms  1 Passed   0 Skipped   0 Failed   //:script_pass
        TESTS FAILED: 1 FAILURE
        Failed target: //:script_fail
        FAIL //:script_fail
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        buck.inject_test_env_arg()
        | {
            "args": attrs.list(
                attrs.arg(),
                default = [],
                doc = """
                The list of arguments to invoke this script with. These are literal values, and no shell interpolation is done.

                 These can contain `string parameter macros`
                , for example, to give the location of a generated binary to the test script.
            """,
            ),
            "deps": attrs.list(attrs.dep(), default = []),
            "env": attrs.dict(
                key = attrs.string(),
                value = attrs.arg(),
                sorted = False,
                default = {},
                doc = """
                Environment variable overrides that should be used when running the script. The key is the variable name, and the value is its value.

                 The values can contain `string parameter macros`
                such as the location of a generated binary to be used by the test script.
            """,
            ),
            "list_args": attrs.option(attrs.list(attrs.string()), default = None),
            "list_env": attrs.option(attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False), default = None),
            "resources": attrs.list(attrs.source(), default = []),
            "run_args": attrs.list(attrs.string(), default = []),
            "run_env": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
            "run_test_separately": attrs.bool(default = False),
            "supports_test_execution_caching": attrs.bool(default = True),
            "test": attrs.option(
                attrs.one_of(attrs.dep(), attrs.source()),
                default = None,
                doc = """
                Either the path to the script (relative to the build file), or a `build target`.
                 This file must be executable in order to be run.
            """,
            ),
            "test_rule_timeout_ms": attrs.option(attrs.int(), default = None),
            "type": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                If provided, this will be sent to any configured `.buckconfig`
            """,
            ),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | test_common.attributes()
        | re_test_common.test_args()
        | constraint_overrides.attributes
    ),
    cfg = constraint_overrides.transition,
)
