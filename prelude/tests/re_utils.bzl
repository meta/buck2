# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//tests:remote_test_execution_toolchain.bzl", "RemoteTestExecutionToolchainInfo")
load("@prelude//utils:expect.bzl", "expect_non_none")
load("@prelude//utils:type_defs.bzl", "type_utils")

ReArg = record(
    disabled = field(bool | None, default = None),
    re_props = field(dict | None, default = None),
    default_run_as_bundle = field(bool | None, default = None),
)

# Result of `get_re_executors_from_props`.
#
# `run_from_project_root` and `use_project_relative_paths` report whether the
# returned executor is remote-execution eligible and therefore the test must run
# from the project root with project-relative paths. They are currently the same
# value, but keep them separate from each other and from "is there an executor at
# all?" so callers can evolve cwd and path behavior independently.
RemoteTestExecutorConfig = record(
    default_executor = field([CommandExecutorConfig, None], default = None),
    executor_overrides = field(dict[str, CommandExecutorConfig], default = {}),
    run_from_project_root = field(bool, default = False),
    use_project_relative_paths = field(bool, default = False),
)

_FORCE_RUN_AS_BUNDLE = read_root_config("tpx", "force_run_as_bundle", "False")

def _force_local_re_tests() -> bool:
    # Must agree with the `read_bool` of this key in
    # `tools/build_defs/fb_native_wrapper.bzl`, which strips `remote_execution` off
    # test targets: when the two disagree a target keeps its RE props but loses its
    # executor, or vice versa. `read_bool` itself is not reusable here because its
    # `read_config` binding is unavailable during analysis, so mirror its coercion —
    # including rejecting values it cannot coerce rather than guessing.
    value = read_root_config("fbcode", "disable_re_tests", "")

    # An empty value means "unset", so that a later config can clear an earlier one.
    if value == "":
        return False
    lowered = value.lower()
    if lowered == "true":
        return True
    if lowered == "false":
        return False
    fail("`fbcode:disable_re_tests`: cannot coerce {!r} to bool".format(value))

def _get_re_arg(ctx: AnalysisContext) -> ReArg:
    if _force_local_re_tests() or not hasattr(ctx.attrs, "remote_execution"):
        # NOTE: this is kinda weird, we take this path if the attr is missing completely
        # Even if the value is None we still follow. Adding force.local to give users
        # some means of bypassing.
        # Example usecase: SGW wants to run kotlin_test targets on MBP/OSX locally
        # eg: buck2 test --local-only -c fbcode.disable_re_tests=True //signals/cloudbridge/v2/libs/cb-meters:test
        return ReArg(re_props = None, default_run_as_bundle = False)

    if ctx.attrs.remote_execution != None:
        if ctx.attrs.remote_execution == "disabled":
            return ReArg(disabled = True)
        elif type_utils.is_string(ctx.attrs.remote_execution):
            # If this is a string, look up the re_props on the RE toolchain.
            expect_non_none(ctx.attrs._remote_test_execution_toolchain)
            return ReArg(
                re_props = ctx.attrs._remote_test_execution_toolchain[RemoteTestExecutionToolchainInfo].profiles[ctx.attrs.remote_execution],
                default_run_as_bundle = ctx.attrs._remote_test_execution_toolchain[RemoteTestExecutionToolchainInfo].default_run_as_bundle,
            )

        return ReArg(re_props = ctx.attrs.remote_execution, default_run_as_bundle = False)

    # Check for a default RE option on the toolchain.
    re_toolchain = ctx.attrs._remote_test_execution_toolchain
    if re_toolchain != None and re_toolchain[RemoteTestExecutionToolchainInfo].default_profile != None:
        return ReArg(
            re_props = re_toolchain[RemoteTestExecutionToolchainInfo].default_profile,
            default_run_as_bundle = re_toolchain[RemoteTestExecutionToolchainInfo].default_run_as_bundle,
        )

    return ReArg(re_props = None, default_run_as_bundle = False)

def maybe_add_run_as_bundle_label(ctx: AnalysisContext, labels: list[str]) -> None:
    if "re_ignore_force_run_as_bundle" in labels:
        return
    re_arg = _get_re_arg(ctx)
    if re_arg.default_run_as_bundle or _FORCE_RUN_AS_BUNDLE == "True":
        labels.extend(["run_as_bundle"])

def get_re_executors_from_props(ctx: AnalysisContext, dynamic_image_override: [dict, None] = None) -> RemoteTestExecutorConfig:
    """
    Convert the `remote_execution` properties param into `CommandExecutorConfig` objects to use with test providers.

    The target's `network_access` policy (if any) is attached to the returned
    `CommandExecutorConfig`(s) so it is enforced for both local and remote test
    execution. When a target has no RE profile, a local-only executor is synthesized
    so Buck does not fall back to a remote-only build execution platform for a test
    whose cell-relative paths make it ineligible for RE. The executor is reported with
    `run_from_project_root = False` so the test keeps running in-place rather than
    being switched to project-root/RE-style execution.

    Args:
        ctx: The analysis context.
        dynamic_image_override: If provided, overrides the `remote_execution_dynamic_image`
            from `remote_execution` props. Use this to inject a resolved snapshotted fbpkg
            image (with pinned uuid) at analysis time.

    Returns a `RemoteTestExecutorConfig`.
    """

    return _get_re_executors(ctx, _get_re_arg(ctx), dynamic_image_override)

def get_re_executors_from_explicit_props(ctx: AnalysisContext, re_props: dict | None) -> RemoteTestExecutorConfig:
    """
    Like `get_re_executors_from_props`, but for a rule that carries RE properties in
    some attribute other than `remote_execution`.

    `re_props` must already be a resolved property dict. The string form of
    `remote_execution` — the name of a profile on the remote test execution toolchain —
    is deliberately not accepted here: resolving one requires
    `_remote_test_execution_toolchain`, and a rule that runs RE only for a secondary
    test (a rustdoc doctest, say) should not have to take a toolchain dependency to
    express that.

    Returns a `RemoteTestExecutorConfig`.
    """

    if _force_local_re_tests():
        re_props = None

    return _get_re_executors(
        ctx,
        ReArg(re_props = re_props, default_run_as_bundle = False),
        None,
    )

def _get_re_executors(
    ctx: AnalysisContext,
    re_arg: ReArg,
    dynamic_image_override: [dict, None],
) -> RemoteTestExecutorConfig:
    network_access = getattr(ctx.attrs, "network_access", None)

    if re_arg.disabled:
        executor = CommandExecutorConfig(local_enabled = True, remote_enabled = False, remote_cache_enabled = False, network_access = network_access)
        # A `remote_execution = "disabled"` target has always produced an executor
        # and therefore run from the project root; preserve that behavior.
        return RemoteTestExecutorConfig(default_executor = executor, run_from_project_root = True, use_project_relative_paths = True)

    re_props = re_arg.re_props
    if re_props == None:
        # A test without an RE profile uses cell-relative paths, which makes the test
        # action local-only. Give it an explicit local executor instead of inheriting
        # the rule's build execution platform: that platform may be remote-only when
        # cross-building, even though the target binary must run on the local host.
        #
        # `remote_cache_enabled = False` keeps this a plain `Executor::Local`
        # (parity: the unset default is True in fbcode but False in OSS), and nothing
        # is uploaded from here anyway (`allow_cache_uploads` is False).
        executor = CommandExecutorConfig(local_enabled = True, remote_enabled = False, remote_cache_enabled = False, network_access = network_access)
        return RemoteTestExecutorConfig(default_executor = executor)

    re_props_copy = dict(re_props)
    missing_props = [name for name in ("capabilities", "use_case") if name not in re_props_copy]
    if missing_props:
        fail("{}: re props are missing required fields: {}".format(ctx.label, ", ".join(missing_props)))
    capabilities = re_props_copy.pop("capabilities")
    use_case = re_props_copy.pop("use_case")
    listing_capabilities = re_props_copy.pop("listing_capabilities", None)
    remote_cache_enabled = re_props_copy.pop("remote_cache_enabled", None)
    remote_enabled = re_props_copy.pop("remote_enabled", True)
    re_dependencies = re_props_copy.pop("dependencies", [])
    re_gang_workers = re_props_copy.pop("gang_workers", [])
    re_gang = re_props_copy.pop("gang", None)
    local_enabled = re_props_copy.pop("local_enabled", False)
    local_listing_enabled = re_props_copy.pop("local_listing_enabled", None)
    if re_props_copy.pop("disable_local_listing", False):
        local_listing_enabled = False
    re_resource_units = re_props_copy.pop("resource_units", None)
    re_listing_resource_units = re_props_copy.pop("listing_resource_units", re_resource_units)
    re_dynamic_image = re_props_copy.pop("remote_execution_dynamic_image", None)
    meta_internal_extra_params = re_props_copy.pop("meta_internal_extra_params", None)
    if dynamic_image_override != None:
        re_dynamic_image = dynamic_image_override
    if re_props_copy:
        unexpected_props = ", ".join(re_props_copy.keys())
        fail("{}: found unexpected re props: {}".format(ctx.label, unexpected_props))
    if not local_enabled and not remote_enabled:
        fail("{}: neither remote nor local test execution are enabled".format(ctx.label))

    if re_gang != None:
        meta_internal_extra_params = dict(meta_internal_extra_params or {})
        meta_internal_extra_params["remote_execution_gang"] = re_gang

    default_executor = CommandExecutorConfig(
        local_enabled = local_enabled,
        remote_enabled = remote_enabled,
        remote_execution_properties = capabilities,
        remote_execution_use_case = use_case or "tpx-default",
        remote_cache_enabled = remote_cache_enabled,
        remote_execution_dependencies = re_dependencies,
        remote_execution_gang_workers = re_gang_workers,
        remote_execution_resource_units = re_resource_units,
        remote_execution_dynamic_image = re_dynamic_image,
        meta_internal_extra_params = meta_internal_extra_params,
        network_access = network_access,
    )

    listing_executor = default_executor
    if listing_capabilities != None or local_listing_enabled != None:
        listing_executor = CommandExecutorConfig(
            local_enabled = local_listing_enabled if local_listing_enabled != None else local_enabled,
            remote_enabled = remote_enabled,
            remote_execution_properties = listing_capabilities if listing_capabilities != None else capabilities,
            remote_execution_use_case = use_case or "tpx-default",
            remote_cache_enabled = remote_cache_enabled,
            remote_execution_resource_units = re_listing_resource_units,
            remote_execution_dynamic_image = re_dynamic_image,
            meta_internal_extra_params = meta_internal_extra_params,
            network_access = network_access,
        )
    return RemoteTestExecutorConfig(
        default_executor = default_executor,
        executor_overrides = {"listing": listing_executor},
        run_from_project_root = True,
        use_project_relative_paths = True,
    )
