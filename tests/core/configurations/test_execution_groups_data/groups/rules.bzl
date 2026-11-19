# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

Resources = exec_group()
Package = exec_group()
Link = exec_group()
Unused = exec_group()
TestRun = exec_group()

ToolPlugin = plugins.kind()

def _tool_impl(ctx):
    script = ctx.actions.write(
        "tool.py",
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "    f.write('{}')".format(ctx.label.name),
        ],
    )
    return [DefaultInfo(default_output = script), RunInfo(args = cmd_args("python3", script))]

tool = rule(impl = _tool_impl, attrs = {})

def _toolchain_impl(ctx):
    return [DefaultInfo(), ctx.attrs.tool[RunInfo]]

toolchain = rule(
    impl = _toolchain_impl,
    attrs = {"tool": attrs.exec_dep(providers = [RunInfo])},
    is_toolchain_rule = True,
)

def _noop_impl(_ctx):
    return [DefaultInfo()]

def _run(ctx, name, tool, exec_group = None, extra_inputs = []):
    out = ctx.actions.declare_output(name + ".txt")
    ctx.actions.run(
        cmd_args(tool[RunInfo], out.as_output(), hidden = extra_inputs),
        category = name,
        exec_group = exec_group,
    )
    return out

# Resources and packaging use toolchains that need different platforms; neither constrains
# the default execution group.
def _bundle_impl(ctx):
    resources = _run(ctx, "compile_resources", ctx.attrs._resource_toolchain, Resources)
    bundle = _run(ctx, "package", ctx.attrs._package_toolchain, Package, [resources])
    return [DefaultInfo(default_output = bundle)]

bundle = rule(
    impl = _bundle_impl,
    attrs = {
        "_package_toolchain": attrs.toolchain_dep(
            default = "//groups:package_toolchain",
            exec_group = Package,
        ),
        "_resource_toolchain": attrs.toolchain_dep(
            default = "//groups:resource_toolchain",
            exec_group = Resources,
        ),
    },
    exec_groups = [Resources, Package],
)

# Compiles remotely and links locally; `Unused` has no deps and still resolves.
def _binary_impl(ctx):
    objects = _run(ctx, "compile", ctx.attrs._compiler)
    explicit_default = _run(ctx, "explicit_default", ctx.attrs._any_tool, DefaultExecGroup)
    link_args = ctx.actions.write("link_args.txt", [objects], exec_group = Link)
    linked = _run(ctx, "link", ctx.attrs._linker, Link, [objects, link_args])
    unused = _run(ctx, "unused_group", ctx.attrs._any_tool_link, Unused)
    return [DefaultInfo(default_outputs = [linked, explicit_default, unused])]

binary = rule(
    impl = _binary_impl,
    attrs = {
        "_any_tool": attrs.exec_dep(default = "//groups:any_tool", exec_group = DefaultExecGroup),
        "_any_tool_link": attrs.exec_dep(default = "//groups:any_tool", exec_group = Link),
        "_compiler": attrs.exec_dep(default = "//groups:remote_tool", exec_group = DefaultExecGroup),
        "_linker": attrs.exec_dep(default = "//groups:any_tool", exec_group = Link),
        "_local_marker": attrs.exec_dep(default = "//groups:local_tool", exec_group = Link),
    },
    exec_groups = [DefaultExecGroup, Link, Unused],
)

# Runs `dynamic_compile` in the default group and `dynamic_link` in `Link` from a dynamic output.
def _dynamic_binary_impl(ctx):
    dynamic_input = ctx.actions.write("dynamic_input.txt", "")
    compiled = ctx.actions.declare_output("dynamic_compile.txt")
    linked = ctx.actions.declare_output("dynamic_link.txt")
    compiler = ctx.attrs._compiler[RunInfo]
    linker = ctx.attrs._linker[RunInfo]

    def f(ctx, _artifacts, outputs):
        ctx.actions.run(cmd_args(compiler, outputs[compiled].as_output()), category = "dynamic_compile")
        ctx.actions.run(
            cmd_args(linker, outputs[linked].as_output()),
            category = "dynamic_link",
            exec_group = Link,
        )

    ctx.actions.dynamic_output(
        dynamic = [dynamic_input],
        inputs = [],
        outputs = [compiled.as_output(), linked.as_output()],
        f = f,
    )
    return [DefaultInfo(default_outputs = [compiled, linked])]

dynamic_binary = rule(
    impl = _dynamic_binary_impl,
    attrs = {
        "_compiler": attrs.exec_dep(default = "//groups:remote_tool", exec_group = DefaultExecGroup),
        "_linker": attrs.exec_dep(default = "//groups:any_tool", exec_group = Link),
        "_local_marker": attrs.exec_dep(default = "//groups:local_tool", exec_group = Link),
    },
    exec_groups = [DefaultExecGroup, Link],
)

# The same tool in the default group and in `Link`, both resolving to the same platform.
same_platform = rule(
    impl = _noop_impl,
    attrs = {
        "_tool": attrs.exec_dep(default = "//groups:any_tool", exec_group = DefaultExecGroup),
        "_tool_in_link": attrs.exec_dep(default = "//groups:any_tool", exec_group = Link),
    },
    exec_groups = [DefaultExecGroup, Link],
)

# The same toolchain in the default group and in `Link`, both resolving to the same platform.
same_platform_toolchain = rule(
    impl = _noop_impl,
    attrs = {
        "_toolchain": attrs.toolchain_dep(default = "//groups:any_toolchain", exec_group = DefaultExecGroup),
        "_toolchain_in_link": attrs.toolchain_dep(default = "//groups:any_toolchain", exec_group = Link),
    },
    exec_groups = [DefaultExecGroup, Link],
)

# Gets `local_tool` as an exec dep from a plugin rather than from an attribute value.
plugin_user = rule(
    impl = _noop_impl,
    attrs = {
        "_linker": attrs.exec_dep(default = "//groups:any_tool", exec_group = Link),
        "_plugin": attrs.plugin_dep(kind = ToolPlugin, default = "//groups:local_tool"),
    },
    exec_groups = [DefaultExecGroup, Link],
    uses_plugins = [ToolPlugin],
)

# The target, not the rule, says what the `Link` platform must satisfy.
constrained_link = rule(
    impl = _noop_impl,
    attrs = {
        "link_constraints": attrs.exec_compatible_with(exec_group = Link, default = []),
        "_compiler": attrs.exec_dep(default = "//groups:remote_tool", exec_group = DefaultExecGroup),
    },
    exec_groups = [DefaultExecGroup, Link],
)

# `default_constraints` constrains only the default group; `Link` resolves on its own.
default_constrained = rule(
    impl = _noop_impl,
    attrs = {
        "default_constraints": attrs.exec_compatible_with(exec_group = DefaultExecGroup, default = []),
        "_linker": attrs.exec_dep(default = "//groups:any_tool", exec_group = Link),
    },
    exec_groups = [DefaultExecGroup, Link],
)

# A rule without named groups constrains its only group, the default, the same way.
default_constrained_no_groups = rule(
    impl = _noop_impl,
    attrs = {
        "default_constraints": attrs.exec_compatible_with(exec_group = DefaultExecGroup, default = []),
    },
)

# Tools named by the macros of an arg in `Link` join `Link`, not the default group.
def _arg_tools_impl(ctx):
    out = ctx.actions.declare_output("arg_tool.txt")
    ctx.actions.run(cmd_args(ctx.attrs.cmd, out.as_output()), category = "arg_tool", exec_group = Link)
    return [DefaultInfo(default_output = out)]

arg_tools = rule(
    impl = _arg_tools_impl,
    attrs = {
        "cmd": attrs.arg(exec_group = Link),
        "_compiler": attrs.exec_dep(default = "//groups:remote_tool", exec_group = DefaultExecGroup),
    },
    exec_groups = [DefaultExecGroup, Link],
)

# A remote-only exec dep keeps the default group remote; the test runs in the local `TestRun`.
def _group_test_impl(_ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            type = "custom",
            command = ["python3", "-c", "pass"],
            exec_group = TestRun,
        ),
    ]

group_test = rule(
    impl = _group_test_impl,
    attrs = {
        "_compiler": attrs.exec_dep(default = "//groups:remote_tool", exec_group = DefaultExecGroup),
        "_runner": attrs.exec_dep(default = "//groups:local_tool", exec_group = TestRun),
    },
    exec_groups = [DefaultExecGroup, TestRun],
)

# Returns the test info of `test` without declaring its group. The remote-only exec dep keeps this
# rule's default group remote, so the test only runs locally if it keeps `TestRun`'s executor.
forwarding_test = rule(
    impl = lambda ctx: [DefaultInfo(), ctx.attrs.test[ExternalRunnerTestInfo]],
    attrs = {
        "test": attrs.dep(),
        "_compiler": attrs.exec_dep(default = "//groups:remote_tool"),
    },
)

def _copying_test_impl(ctx):
    info = ctx.attrs.test[ExternalRunnerTestInfo]
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            type = info.test_type,
            command = info.command,
            default_executor = info.default_executor,
            exec_group = info.exec_group,
        ),
    ]

# Re-creates the test info of `test` field by field without declaring its group: the recorded
# `default_executor` wins over `exec_group`, so the test keeps `TestRun`'s executor.
copying_test = rule(
    impl = _copying_test_impl,
    attrs = {
        "test": attrs.dep(),
        "_compiler": attrs.exec_dep(default = "//groups:remote_tool"),
    },
)

def _anon_runner_impl(ctx):
    out = ctx.actions.declare_output("anon_link.txt", has_content_based_path = False)
    ctx.actions.run(cmd_args(ctx.attrs.tool[RunInfo], out.as_output()), category = "anon_link")
    return [DefaultInfo(default_output = out)]

_anon_runner = anon_rule(
    impl = _anon_runner_impl,
    attrs = {"tool": attrs.exec_dep()},
    artifact_promise_mappings = {"out": lambda p: p[DefaultInfo].default_outputs[0]},
)

# Passes the `Link` tool to an anon target created in `Link`; the default group is remote-only.
def _anon_in_group_impl(ctx):
    attrs = {"tool": ctx.attrs._linker}
    group = Link if ctx.attrs.in_group else None
    if ctx.attrs.plural:
        anon = ctx.actions.anon_targets([(_anon_runner, attrs)], exec_group = group).anon_targets[0]
    else:
        anon = ctx.actions.anon_target(_anon_runner, attrs, exec_group = group)
    return [DefaultInfo(default_output = anon.artifact("out"))]

anon_in_group = rule(
    impl = _anon_in_group_impl,
    attrs = {
        "in_group": attrs.bool(default = True),
        "plural": attrs.bool(default = False),
        "_compiler": attrs.exec_dep(default = "//groups:remote_tool", exec_group = DefaultExecGroup),
        "_linker": attrs.exec_dep(default = "//groups:local_tool", exec_group = Link),
    },
    exec_groups = [DefaultExecGroup, Link],
)

def _anon_arg_runner_impl(ctx):
    out = ctx.actions.declare_output("anon_arg.txt", has_content_based_path = False)
    # `$(exe)` expands to one string, `python3 <script>`, so a shell splits it.
    script = cmd_args(ctx.attrs.cmd, '"$0"', delimiter = " ")
    ctx.actions.run(cmd_args("sh", "-c", script, out.as_output()), category = "anon_arg")
    return [DefaultInfo(default_output = out)]

_anon_arg_runner = anon_rule(
    impl = _anon_arg_runner_impl,
    attrs = {"cmd": attrs.arg(anon_target_compatible = True)},
    artifact_promise_mappings = {"out": lambda p: p[DefaultInfo].default_outputs[0]},
)

# Passes the tool of `default_cmd` or `link_cmd` to an anon target created in the default group or `Link`.
def _anon_arg_impl(ctx):
    cmd = ctx.attrs.link_cmd if ctx.attrs.use_link_cmd else ctx.attrs.default_cmd
    group = Link if ctx.attrs.in_link else None
    anon = ctx.actions.anon_target(_anon_arg_runner, {"cmd": cmd}, exec_group = group)
    return [DefaultInfo(default_output = anon.artifact("out"))]

anon_arg = rule(
    impl = _anon_arg_impl,
    attrs = {
        "default_cmd": attrs.arg(anon_target_compatible = True, exec_group = DefaultExecGroup),
        "in_link": attrs.bool(),
        "link_cmd": attrs.arg(anon_target_compatible = True, exec_group = Link),
        "use_link_cmd": attrs.bool(),
    },
    exec_groups = [DefaultExecGroup, Link],
)

# `exec_group = None` is the default group, so wrappers can pass an optional group through.
plain = rule(impl = _noop_impl, attrs = {"_tool": attrs.exec_dep(default = "//groups:any_tool", exec_group = None)})

ArgToolchainInfo = provider(fields = ["cmd"])

def _arg_toolchain_impl(ctx):
    return [DefaultInfo(), ArgToolchainInfo(cmd = ctx.attrs.cmd)]

# A toolchain whose arg refers to a tool, configured for the platform of the group that uses it.
arg_toolchain = rule(
    impl = _arg_toolchain_impl,
    attrs = {"cmd": attrs.arg(anon_target_compatible = True)},
    is_toolchain_rule = True,
)

# Passes the arg of a `Link` toolchain to an anon target created in `Link` or the default group.
def _anon_toolchain_arg_impl(ctx):
    cmd = ctx.attrs._toolchain[ArgToolchainInfo].cmd
    group = Link if ctx.attrs.in_link else None
    anon = ctx.actions.anon_target(_anon_arg_runner, {"cmd": cmd}, exec_group = group)
    return [DefaultInfo(default_output = anon.artifact("out"))]

anon_toolchain_arg = rule(
    impl = _anon_toolchain_arg_impl,
    attrs = {
        "in_link": attrs.bool(),
        "_compiler": attrs.exec_dep(default = "//groups:remote_tool", exec_group = DefaultExecGroup),
        "_toolchain": attrs.toolchain_dep(default = "//groups:local_arg_toolchain", exec_group = Link),
    },
    exec_groups = [DefaultExecGroup, Link],
)
