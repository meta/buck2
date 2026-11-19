# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

Link = exec_group()

tool = rule(impl = lambda ctx: [DefaultInfo(), RunInfo(args = cmd_args("true"))], attrs = {})

_takes_tool = anon_rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {"tool": attrs.exec_dep()},
    artifact_promise_mappings = {},
)

# The `Link` tool resolves to the same platform as the default group, so an anon target created
# on the default platform accepts it.
def _impl(ctx):
    ctx.actions.anon_target(_takes_tool, {"tool": ctx.attrs._tool_in_link})
    return [DefaultInfo()]

passes_group_tool = rule(
    impl = _impl,
    attrs = {"_tool_in_link": attrs.exec_dep(default = "//anon_tool:tool", exec_group = Link)},
    exec_groups = [DefaultExecGroup, Link],
)
