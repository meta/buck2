# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_TEST_ALLOW_CACHE_UPLOADS = read_config("test", "allow_cache_uploads", "true")

def _write_impl(ctx):
    out = ctx.actions.write("out.txt", ctx.attrs.content, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

write = rule(
    impl = _write_impl,
    attrs = {
        "content": attrs.string(),
    },
)

ArgsTSet = transitive_set(args_projections = {"args": lambda v: v})

def _write_tset_impl(ctx):
    child = ctx.actions.tset(ArgsTSet, value = cmd_args("child"))
    tset = ctx.actions.tset(ArgsTSet, value = cmd_args(ctx.attrs.content), children = [child])
    out = ctx.actions.write("out.txt", tset.project_as_args("args"), has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

# Writes a transitive set projection: the content, then "child".
write_tset = rule(
    impl = _write_tset_impl,
    attrs = {
        "content": attrs.string(),
    },
)

def _execution_platforms(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [
                ExecutionPlatformInfo(
                    label = ctx.label.raw_target(),
                    configuration = ConfigurationInfo(constraints = {}, values = {}),
                    # The same separator on every host: the write cache key covers it, and the
                    # golden keys below are read on every host.
                    executor_config = CommandExecutorConfig(
                        local_enabled = True,
                        remote_enabled = False,
                        use_windows_path_separators = False,
                        # Whether local results are published, which is what lets a write's
                        # content be recorded.
                        allow_cache_uploads = _TEST_ALLOW_CACHE_UPLOADS in ["true", "True"],
                    ),
                ),
            ],
        ),
    ]

execution_platforms = rule(
    impl = _execution_platforms,
    attrs = {},
)
