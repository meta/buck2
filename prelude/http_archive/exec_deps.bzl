# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

HttpArchiveExecDeps = provider(
    fields = {
        "exec_os_type": provider_field(typing.Any, default = None),
    }
)

def _http_archive_exec_deps_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        HttpArchiveExecDeps(
            exec_os_type = ctx.attrs.exec_os_type,
        ),
    ]

http_archive_exec_deps = rule(
    impl = _http_archive_exec_deps_impl,
    attrs = {
        "exec_os_type": attrs.default_only(attrs.dep(default = "prelude//os_lookup/targets:os_lookup")),
    },
)
