# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

def _impl(_ctx):
    return [DefaultInfo(), ExternalRunnerTestInfo(type = "custom", command = ["true"])]

r = rule(impl = _impl, attrs = {}, exec_groups = [G])

def _explicit_default_impl(_ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(type = "custom", command = ["true"], exec_group = DefaultExecGroup),
    ]

explicit_default = rule(impl = _explicit_default_impl, attrs = {}, exec_groups = [G])

def _overrides_only_impl(_ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            type = "custom",
            command = ["true"],
            executor_overrides = {"listing": CommandExecutorConfig(local_enabled = True, remote_enabled = False)},
        ),
    ]

# The tests run in the default group whenever no override is picked.
overrides_only = rule(impl = _overrides_only_impl, attrs = {}, exec_groups = [G])

# Returns the test info of `test`, as an alias does.
forwarding = rule(impl = lambda ctx: [DefaultInfo(), ctx.attrs.test[ExternalRunnerTestInfo]], attrs = {"test": attrs.dep()})

groupless = rule(impl = _impl, attrs = {})

# Returns the test info that a rule without named groups creates.
grouped_forwarding = rule(
    impl = lambda ctx: [DefaultInfo(), ctx.attrs.test[ExternalRunnerTestInfo]],
    attrs = {"test": attrs.dep()},
    exec_groups = [G],
)
