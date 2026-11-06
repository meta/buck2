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

def _execution_platforms(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [
                ExecutionPlatformInfo(
                    label = ctx.attrs.platform.label.raw_target(),
                    configuration = ctx.attrs.platform[PlatformInfo].configuration,
                    executor_config = CommandExecutorConfig(local_enabled = True, remote_enabled = False),
                ),
            ]
        ),
    ]

execution_platforms = rule(
    impl = _execution_platforms,
    attrs = {"platform": attrs.dep(providers = [PlatformInfo])},
)

def _fan4(platform, refs):
    out = {}
    for cpu in ["x86", "arm"]:
        for variant in ["a", "b"]:
            values = [getattr(refs, cpu)[ConstraintValueInfo], getattr(refs, variant)[ConstraintValueInfo]]
            out[cpu + "_" + variant] = PlatformInfo(
                label = cpu + "_" + variant,
                configuration = ConfigurationInfo(
                    constraints = {v.setting.label: v for v in values},
                    values = {},
                ),
            )
    return out

_FAN_REFS = {"a": "root//:a", "arm": "root//:arm", "b": "root//:b", "x86": "root//:x86"}

fan4 = transition(impl = _fan4, refs = _FAN_REFS, split = True)

def _only(cpu):
    def impl(platform, refs):
        return {k: v for k, v in _fan4(platform, refs).items() if k.startswith(cpu + "_")}

    return impl

fan_x86_cfg = transition(impl = _only("x86"), refs = _FAN_REFS, split = True)
fan_arm_cfg = transition(impl = _only("arm"), refs = _FAN_REFS, split = True)

def _fan(ctx):
    outs = {}
    for d in ctx.attrs.deps:
        for cfg, dep in d.items():
            out = dep[DefaultInfo].default_outputs[0]
            outs[dep.label.name + "." + cfg] = out
    return [DefaultInfo(default_output = ctx.actions.symlinked_dir("all", outs))]

fan = rule(impl = _fan, attrs = {"deps": attrs.list(attrs.split_transition_dep(cfg = fan4))})
fan_x86 = rule(impl = _fan, attrs = {"deps": attrs.list(attrs.split_transition_dep(cfg = fan_x86_cfg))})
fan_arm = rule(impl = _fan, attrs = {"deps": attrs.list(attrs.split_transition_dep(cfg = fan_arm_cfg))})

def _gen(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = True)
    script = 'echo "$1" >> "$2"; sleep "$3"; [ "$4" = kill ] && kill -KILL $$; [ "$4" = ok ] && printf "%s\\n" "$5" > "$6"'
    ctx.actions.run(
        cmd_args("sh", "-c", script, "--", ctx.label.name, ctx.attrs.runs, ctx.attrs.delay, ctx.attrs.status, ctx.attrs.text, out.as_output()),
        category = "gen",
        allow_offline_output_cache = ctx.attrs.offline,
        timeout_seconds = ctx.attrs.timeout,
    )
    return [DefaultInfo(default_output = out)]

gen = rule(
    impl = _gen,
    attrs = {
        "delay": attrs.string(default = "3"),
        "offline": attrs.bool(default = False),
        "runs": attrs.string(),
        "status": attrs.string(default = "ok"),
        "text": attrs.string(),
        "timeout": attrs.option(attrs.int(), default = None),
    },
)

# Fails after writing a diagnostic that its error handler reads back as a sub-error category.
def _handler(ctx):
    subs = [ctx.new_sub_error(category = "diag_" + v.read_string().strip(), show_in_stderr = True) for v in ctx.output_artifacts.values()]
    return subs or [ctx.new_sub_error(category = "diag_missing", show_in_stderr = True)]

def _diagnosed(ctx):
    diag = ctx.actions.declare_output("diag.txt", has_content_based_path = True)
    out = ctx.actions.declare_output("out.txt", has_content_based_path = True)
    ctx.actions.run(
        cmd_args(
            "sh", "-c", 'echo "$1" >> "$2"; sleep 3; printf E42 > "$3"; exit 1', "--", ctx.label.name, ctx.attrs.runs, diag.as_output(), hidden = out.as_output()
        ),
        category = "gen",
        error_handler = _handler,
        outputs_for_error_handler = [diag.as_output()],
    )
    return [DefaultInfo(default_output = out)]

diagnosed = rule(impl = _diagnosed, attrs = {"runs": attrs.string()})

# Copies `used` and lists only it in its dep file, so a change to `unused` alone is a dep file hit.
def _depf(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = True)
    dep_file = ctx.actions.declare_output("dep.txt", has_content_based_path = True)
    tag = ctx.actions.artifact_tag()
    unused = ctx.attrs.unused[DefaultInfo].default_outputs[0]
    script = 'echo "$1" >> "$2"; sleep 2; cat "$3" > "$5"; printf "%s\\n" "${3#./}" > "$6"'
    ctx.actions.run(
        cmd_args(
            "sh",
            "-c",
            script,
            "--",
            ctx.label.name,
            ctx.attrs.runs,
            tag.tag_artifacts(ctx.attrs.used),
            tag.tag_artifacts(unused),
            out.as_output(),
            tag.tag_artifacts(dep_file.as_output()),
        ),
        category = "depf",
        dep_files = {"inputs": tag},
    )
    return [DefaultInfo(default_output = out)]

depf = rule(
    impl = _depf,
    attrs = {
        "runs": attrs.string(),
        "unused": attrs.dep(),
        "used": attrs.source(),
    },
)
