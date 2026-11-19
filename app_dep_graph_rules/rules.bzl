# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Avoid some copy-paste
def _app(s):
    return "//buck2/app/" + s + ":" + s

# These crates should only implement late bindings and not be depended on
# directly
LATE_BINDING_ONLY_CRATES = [
    _app("buck2_anon_target"),
    _app("buck2_cmd_audit_server"),
    _app("buck2_cmd_lsp_server"),
    _app("buck2_cmd_query_server"),
    _app("buck2_cmd_targets_server"),
    _app("buck2_bxl"),
    _app("buck2_query_impls"),
]

# These crates may only be depended on from `app/buck2`
TOP_LEVEL_ONLY_CRATES = [
    _app("buck2_cmd_debug_client"),
    _app("buck2_cmd_log_client"),
]

# Unordered pairs where neither crate may depend on the other
BANNED_DEP_PATHS = [
    (_app("buck2_action_impl"), _app("buck2_analysis")),
    (_app("buck2_action_impl"), _app("buck2_bxl")),
    (_app("buck2_cli_proto"), _app("buck2_core")),
    (_app("buck2_common"), _app("buck2_directory")),
    (_app("buck2_common"), "//buck2/starlark-rust/starlark:starlark"),
    (_app("buck2_build_api"), _app("buck2_execute_impl")),
    (_app("buck2_build_api"), _app("buck2_interpreter_for_build")),
    (_app("buck2_server"), _app("buck2_server_commands")),
    (_app("buck2_bxl"), _app("buck2_configured")),
]

# Ordered pairs `(a, b)` where `a` may not depend on `b`, while `b` may depend on `a`
BANNED_DIRECTED_DEP_PATHS = [
    (_app("buck2_artifact"), _app("buck2_execute")),
]
