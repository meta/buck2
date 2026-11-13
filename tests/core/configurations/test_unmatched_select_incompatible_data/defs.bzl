def _constraint_setting(ctx):
    return [
        DefaultInfo(),
        ConstraintSettingInfo(
            label = ctx.label.raw_target(),
            default = ctx.attrs.default.raw_target().with_sub_target() if ctx.attrs.default else None,
        ),
    ]

constraint_setting = rule(
    impl = _constraint_setting,
    is_configuration_rule = True,
    attrs = {"default": attrs.option(attrs.label(), default = None)},
)

def _constraint_value(ctx):
    cv = ConstraintValueInfo(setting = ctx.attrs.setting[ConstraintSettingInfo], label = ctx.label.raw_target())
    return [DefaultInfo(), cv, ConfigurationInfo(constraints = {cv.setting.label: cv}, values = {})]

constraint_value = rule(
    impl = _constraint_value,
    is_configuration_rule = True,
    attrs = {"setting": attrs.dep(providers = [ConstraintSettingInfo])},
)

def _constraints(values):
    return {v[ConstraintValueInfo].setting.label: v[ConstraintValueInfo] for v in values}

def _config_setting(ctx):
    return [DefaultInfo(), ConfigurationInfo(constraints = _constraints(ctx.attrs.values), values = {})]

config_setting = rule(
    impl = _config_setting,
    is_configuration_rule = True,
    attrs = {"values": attrs.list(attrs.dep(providers = [ConstraintValueInfo]))},
)

def _platform(ctx):
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            configuration = ConfigurationInfo(constraints = _constraints(ctx.attrs.values), values = {}),
        ),
    ]

platform = rule(
    impl = _platform,
    is_configuration_rule = True,
    attrs = {"values": attrs.list(attrs.dep(providers = [ConstraintValueInfo]))},
)

def _execution_platforms(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [
                ExecutionPlatformInfo(
                    label = p.label.raw_target(),
                    configuration = p[PlatformInfo].configuration,
                    executor_config = CommandExecutorConfig(local_enabled = True, remote_enabled = False),
                )
                for p in ctx.attrs.platforms
            ],
        ),
    ]

execution_platforms = rule(
    impl = _execution_platforms,
    attrs = {"platforms": attrs.list(attrs.dep(providers = [PlatformInfo]))},
)

TextInfo = provider(fields = ["text"])

def _lib(ctx):
    parts = [ctx.attrs.value]
    for d in ctx.attrs.deps:
        parts.append(d[TextInfo].text)
    if ctx.attrs.toolchain:
        parts.append(ctx.attrs.toolchain[TextInfo].text)
    text = ctx.label.name + "(" + ",".join(parts) + ")"
    return [
        DefaultInfo(default_output = ctx.actions.write("out.txt", text)),
        TextInfo(text = text),
    ]

_LIB_ATTRS = {
    "deps": attrs.list(attrs.dep(), default = []),
    "toolchain": attrs.option(attrs.toolchain_dep(), default = None),
    "value": attrs.string(default = ""),
}

lib = rule(impl = _lib, attrs = _LIB_ATTRS)

toolchain = rule(impl = _lib, attrs = _LIB_ATTRS, is_toolchain_rule = True)

def _runner(ctx):
    return [DefaultInfo(default_output = ctx.actions.write("out.txt", ctx.attrs.tool[TextInfo].text))]

runner = rule(impl = _runner, attrs = {"tool": attrs.exec_dep(providers = [TextInfo])})
