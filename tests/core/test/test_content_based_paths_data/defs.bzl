# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _run_test_with_content_based_path_impl(ctx):
    unused_but_path_needs_resolving = ctx.actions.declare_output("unused.txt", has_content_based_path = True)
    ctx.actions.write(unused_but_path_needs_resolving, "unused")

    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            command = ["fbpython", "-c", "import sys; sys.exit(0)", cmd_args(unused_but_path_needs_resolving)],
            type = "custom",
        ),
    ]

run_test_with_content_based_path = rule(
    impl = _run_test_with_content_based_path_impl,
    attrs = {},
)

def _broker_impl(ctx):
    json = ctx.actions.declare_output("resources.json", has_content_based_path = True)
    json = ctx.actions.write_json(
        json,
        {
            "resources": [{"my_alias": "42"}],
        },
    )
    return [
        DefaultInfo(),
        LocalResourceInfo(
            setup = cmd_args(["cat", json]),
            resource_env_vars = {
                "MY_RESOURCE_ID": "my_alias",
            },
            setup_timeout_seconds = 5,
        ),
    ]

_broker_attrs = {}

broker = rule(impl = _broker_impl, attrs = _broker_attrs)

def _local_resources_test_impl(ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            type = "custom",
            command = ["true"],
            local_resources = {
                "my_resource_type": ctx.attrs.broker.label,
            },
            required_local_resources = [
                RequiredTestLocalResource("my_resource_type"),
            ],
        ),
    ]

local_resources_test = rule(
    impl = _local_resources_test_impl,
    attrs = {
        "broker": attrs.dep(providers = [LocalResourceInfo]),
    },
)
