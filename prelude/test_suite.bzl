# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")

def test_suite_impl(ctx: AnalysisContext) -> list[Provider]:
    # There is nothing to implement here: test_suite exists as a mechanism to "group" tests using
    # the `tests` attribute, and the `tests` attribute is supported for all rules.
    # We fill in the DefaultInfo so building the test_suite will build all the underlying test rules.
    test_targets = []
    other_outputs = []
    for test_dep in ctx.attrs.test_deps:
        test_targets.append(test_dep.label.raw_target())
        default_info = test_dep[DefaultInfo]
        other_outputs.extend(default_info.default_outputs)
        other_outputs.extend(default_info.other_outputs)
    return [DefaultInfo(default_outputs = [ctx.actions.write("test_targets.txt", test_targets, has_content_based_path = False)], other_outputs = other_outputs)]

test_suite = prelude_rule(
    name = "test_suite",
    impl = test_suite_impl,
    docs = """
        A `test_suite()` is used to create a grouping of tests that should all be run by just testing this rule.

        This rule can then be given to `buck test`, and all tests that it depends on will be invoked.
         Note that the test\\_suite() target is not tested itself, it just tells buck to run other
         tests. It will not show up in calls to the external runner nor in the normal test output.
    """,
    examples = """
        This test\\_suite() sets up two different sets of tests to run, 'all' tests and 'slow' tests. Note that `all_tests` can depend on `slow_tests`, and all three tests are run.

        ```
        # instrumentation_tests/BUCK:
        sh_test(
            name = "instrumentation_tests",
            test = "instrumentation_tests.sh",
            visibility = ["PUBLIC"],
        )

        # integration_tests/BUCK:
        sh_test(
            name = "integration_tests",
            test = "integration_tests.sh",
            visibility = ["PUBLIC"],
        )

        # unit_tests/BUCK:
        sh_test(
            name = "unit_tests",
            test = "unit_tests.sh",
            visibility = ["PUBLIC"],
        )

        # BUCK:
        test_suite(
            name = "slow_tests",
            tests = [
                "//instrumentation_tests:instrumentation_tests",
                "//integration_tests:integration_tests",
            ],
        )

        test_suite(
            name = "all_tests",
            tests = [
                ":slow_tests",
                "//unit_tests:unit_tests",
            ],
        )
        ```

        Yields output like this when run:

        ```
        $ buck test //:slow_tests
        ...
        RESULTS FOR //instrumentation_tests:instrumentation_tests //integration_tests:integration_tests
        PASS    <100ms  1 Passed   0 Skipped   0 Failed   //instrumentation_tests:instrumentation_tests
        PASS    <100ms  1 Passed   0 Skipped   0 Failed   //integration_tests:integration_tests
        TESTS PASSED
        ...

        $ buck test //:all_tests
        RESULTS FOR //instrumentation_tests:instrumentation_tests //integration_tests:integration_tests //unit_tests:unit_tests
        PASS    <100ms  1 Passed   0 Skipped   0 Failed   //instrumentation_tests:instrumentation_tests
        PASS    <100ms  1 Passed   0 Skipped   0 Failed   //integration_tests:integration_tests
        PASS    <100ms  1 Passed   0 Skipped   0 Failed   //unit_tests:unit_tests
        TESTS PASSED
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            # On buck1 query, tests attribute on test_suite is treated as deps, while on buck2 it is not.
            # While buck2's behavior makes more sense, we want to preserve buck1 behavior on test_suite for now to make TD behavior match between buck1 and buck2.
            # This diff makes the behaviors match by adding a test_deps attribute to test_suite on buck2 that is used as a deps attribute. In the macro layer, we set test_deps = tests if we are using buck2.
            # For more context: https://fb.prod.workplace.com/groups/603286664133355/posts/682567096205311/?comment_id=682623719532982&reply_comment_id=682650609530293
            "test_deps": attrs.list(attrs.dep(), default = []),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
