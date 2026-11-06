def _constraint_setting(ctx):
    return [DefaultInfo(), ConstraintSettingInfo(label = ctx.label.raw_target())]

constraint_setting = rule(impl = _constraint_setting, is_configuration_rule = True, attrs = {})

def _constraint_value(ctx):
    cv = ConstraintValueInfo(setting = ctx.attrs.setting[ConstraintSettingInfo], label = ctx.label.raw_target())
    return [DefaultInfo(), cv, ConfigurationInfo(constraints = {cv.setting.label: cv}, values = {})]

constraint_value = rule(
    impl = _constraint_value,
    is_configuration_rule = True,
    attrs = {"setting": attrs.dep(providers = [ConstraintSettingInfo])},
)

def _platform(ctx):
    constraints = {v[ConstraintValueInfo].setting.label: v[ConstraintValueInfo] for v in ctx.attrs.values}
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            configuration = ConfigurationInfo(constraints = constraints, values = {}),
        ),
    ]

platform = rule(
    impl = _platform,
    is_configuration_rule = True,
    attrs = {"values": attrs.list(attrs.dep(providers = [ConstraintValueInfo]), default = [])},
)

# One local execution platform per cpu. Their executor configs differ in whether they may use
# persistent workers, which changes how a command can run, as two RE platforms with different
# images would.
def _execution_platforms(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [
                ExecutionPlatformInfo(
                    label = p.label.raw_target(),
                    configuration = p[PlatformInfo].configuration,
                    executor_config = CommandExecutorConfig(
                        local_enabled = True,
                        remote_enabled = False,
                        use_persistent_workers = p.label.name == "exec_arm",
                    ),
                )
                for p in ctx.attrs.platforms
            ]
        ),
    ]

execution_platforms = rule(
    impl = _execution_platforms,
    attrs = {"platforms": attrs.list(attrs.dep(providers = [PlatformInfo]))},
)

# The same command line and inputs in every configuration.
def _gen(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = True)
    ctx.actions.run(
        cmd_args("sh", "-c", 'echo run >> "$1"; echo gen > "$2"', "--", ctx.attrs.runs, out.as_output()),
        category = "gen",
    )
    return [DefaultInfo(default_output = out)]

gen = rule(impl = _gen, attrs = {"runs": attrs.string()})

# The same bytes, executable or not; `ran` produces them with a command, `written` with a write.
def _ran(ctx):
    out = ctx.actions.declare_output("tool.sh", has_content_based_path = True)
    ctx.actions.run(
        cmd_args("sh", "-c", 'echo tool > "$1"; [ "$2" = no ] || chmod +x "$1"', "--", out.as_output(), "yes" if ctx.attrs.exe else "no"),
        category = "ran",
    )
    return [DefaultInfo(default_output = out)]

def _written(ctx):
    out = ctx.actions.declare_output("tool.sh", has_content_based_path = True)
    ctx.actions.write(out, "tool\n", is_executable = ctx.attrs.exe)
    return [DefaultInfo(default_output = out)]

ran = rule(impl = _ran, attrs = {"exe": attrs.bool()})
written = rule(impl = _written, attrs = {"exe": attrs.bool()})
