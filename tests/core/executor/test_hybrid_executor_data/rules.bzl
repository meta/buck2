# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

ExecutorConfigInfo = provider(fields = ["config"])

def _platform(ctx):
    # We need to introduce a constraint to ensure our different execution
    # platforms are distinct. This is because exec_compatible_with selects a
    # ConfigurationInfo (which provides a config), not a ExecutionPlatformInfo
    # (instead it matches on it).
    configuration = ConfigurationInfo(
        constraints = {
            ctx.attrs.setting.label.raw_target(): ConstraintValueInfo(
                setting = ctx.attrs.setting[ConstraintSettingInfo],
                label = ctx.label.raw_target(),
            ),
        },
        values = {},
    )

    platform = ExecutionPlatformInfo(
        label = ctx.label.raw_target(),
        configuration = configuration,
        executor_config = CommandExecutorConfig(
            local_enabled = ctx.attrs.local_enabled,
            remote_enabled = ctx.attrs.remote_enabled,
            remote_execution_properties = {
                "platform": "linux-remote-execution",
            },
            remote_execution_max_input_files_mebibytes = 1,
            use_limited_hybrid = ctx.attrs.use_limited_hybrid,
            allow_limited_hybrid_fallbacks = ctx.attrs.allow_hybrid_fallbacks_on_failure,
            allow_hybrid_fallbacks_on_failure = ctx.attrs.allow_hybrid_fallbacks_on_failure,
            remote_execution_use_case = "buck2-testing",
            allow_cache_uploads = ctx.attrs.allow_cache_uploads,
            remote_dep_file_cache_enabled = ctx.attrs.remote_dep_file_cache_enabled,
            experimental_low_pass_filter = ctx.attrs.experimental_low_pass_filter,
            max_cache_upload_mebibytes = 1,
        ),
    )

    return [
        DefaultInfo(),
        platform,
        configuration,
    ]

platform = rule(
    impl = _platform,
    attrs = {
        "allow_cache_uploads": attrs.bool(default = False),
        "allow_hybrid_fallbacks_on_failure": attrs.bool(default = False),
        "experimental_low_pass_filter": attrs.bool(
            default = read_config("test", "experimental_low_pass_filter", "") in ["true", "True"],
        ),
        "local_enabled": attrs.bool(default = True),
        "remote_dep_file_cache_enabled": attrs.bool(default = False),
        "remote_enabled": attrs.bool(default = True),
        "setting": attrs.configuration_label(),
        "use_limited_hybrid": attrs.bool(default = True),
    },
)

def _platforms(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [x[ExecutionPlatformInfo] for x in ctx.attrs.platforms],
        ),
    ]

platforms = rule(
    impl = _platforms,
    attrs = {
        "platforms": attrs.list(attrs.dep(providers = [ExecutionPlatformInfo])),
    },
)

def _target_platform(ctx):
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            configuration = ConfigurationInfo(constraints = {}, values = {}),
        ),
    ]

target_platform = rule(
    impl = _target_platform,
    attrs = {},
)

def _config_setting(ctx):
    return [DefaultInfo(), ConstraintSettingInfo(label = ctx.label.raw_target())]

config_setting = rule(
    impl = _config_setting,
    attrs = {},
)

def _file_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    script = "import os; import sys; f = open(sys.argv[2], 'wb'); f.write(os.urandom(int(sys.argv[1]))); f.close()"
    if ctx.attrs.executable:
        script += "; os.chmod(sys.argv[2], 0o755)"

    ctx.actions.run(
        [
            "fbpython",
            "-c",
            script,
            str(ctx.attrs.file_size),
            out.as_output(),
        ],
        category = "head",
        env = {"cache_buster": ctx.attrs.cache_buster},
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

file = rule(
    impl = _file_impl,
    attrs = {
        "cache_buster": attrs.string(default = read_config("test", "cache_buster", "")),
        "executable": attrs.bool(default = False),
        "file_size": attrs.int(),
    },
)

def _cp_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            "-c",
            "import shutil; import sys; shutil.copy(sys.argv[1], sys.argv[2]);",
            ctx.attrs.file[DefaultInfo].default_outputs[0],
            out.as_output(),
        ],
        category = "cp",
        env = {"cache_buster": ctx.attrs.cache_buster},
    )
    return [DefaultInfo(default_output = out)]

cp = rule(
    impl = _cp_impl,
    attrs = {
        "cache_buster": attrs.string(default = read_config("test", "cache_buster", "")),
        "file": attrs.dep(),
    },
)

def _symlink_impl(ctx):
    link = ctx.actions.declare_output("link", has_content_based_path = False)
    target = ctx.actions.declare_output("target", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            "-c",
            "import os; import shutil; import sys; shutil.copy(sys.argv[1], sys.argv[2]); os.symlink(os.path.relpath(sys.argv[2], os.path.dirname(sys.argv[3])), sys.argv[3]);",
            ctx.attrs.source[DefaultInfo].default_outputs[0],
            target.as_output(),
            link.as_output(),
        ],
        category = "symlink",
        env = {"cache_buster": ctx.attrs.cache_buster},
    )

    return [DefaultInfo(default_output = link)]

symlink = rule(
    impl = _symlink_impl,
    attrs = {
        # @lint-ignore BUCKRESTRICTEDSYNTAX
        "cache_buster": attrs.string(default = read_config("test", "cache_buster", "")),
        "source": attrs.dep(),
    },
)

def _command_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            ctx.attrs.command,
            out.as_output(),
        ],
        category = "command",
        env = {"cache_buster": ctx.attrs.cache_buster} | ctx.attrs.env,
        prefer_local = ctx.attrs.prefer_local,
        local_only = ctx.attrs.local_only,
        prefer_remote = ctx.attrs.prefer_remote,
        force_full_hybrid_if_capable = ctx.attrs.force_full_hybrid_if_capable,
        weight = ctx.attrs.weight,
        unique_input_inodes = ctx.attrs.unique_input_inodes,
        allow_cache_upload = ctx.attrs.allow_cache_upload,
    )
    return [DefaultInfo(default_output = out)]

command = rule(
    impl = _command_impl,
    attrs = {
        "allow_cache_upload": attrs.bool(default = False),
        "cache_buster": attrs.string(default = read_config("test", "cache_buster", "")),
        "command": attrs.source(),
        "env": attrs.dict(attrs.string(), attrs.arg(), default = {}),
        "force_full_hybrid_if_capable": attrs.bool(default = False),
        "local_only": attrs.bool(default = False),
        "prefer_local": attrs.bool(default = False),
        "prefer_remote": attrs.bool(default = False),
        "unique_input_inodes": attrs.bool(default = False),
        "weight": attrs.int(default = 1),
    },
)
