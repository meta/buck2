# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbcode//buck2/app:modifier.bzl", "buck2_modifiers")
load("@fbcode_macros//build_defs:native_rules.bzl", "buck_filegroup")
load("@fbcode_macros//build_defs:platform_utils.bzl", "platform_utils")
load("@fbcode_macros//build_defs:python_unittest.bzl", "python_unittest")
load("@fbcode_macros//build_defs/lib:noop_rule.bzl", "is_noop_rule")
load("@fbcode_macros//build_defs/lib:re_test_utils.bzl", "re_test_utils")
load("@fbsource//tools/build_defs:rust_unittest.bzl", "rust_unittest")
load("@fbsource//tools/build_defs/testinfra:network_access_utils.bzl", "network_access_utils")
load("@fbsource//tools/build_defs/xplat:shims.bzl", "xplat_shims")
load("@fbsource//tools/target_determinator/macros:ci.bzl", "ci")
load("@fbsource//tools/target_determinator/macros:ci_hint.bzl", "ci_hint")

def buck_e2e_test(
    name,
    executable,
    use_buck_api = True,
    contacts = None,
    base_module = None,
    data = None,
    data_dir = None,
    srcs = None,
    labels = None,
    deps = None,
    env = None,
    resources = None,
    skip_for_os = (),
    serialize_test_cases = None,
    require_nano_prelude = None,
    cfg_modifiers = None,
    ci_srcs = [],
    ci_deps = [],
    compatible_with = None,
    heavyweight_threads = "4",
    remote_execution = None,
    network_access = None,
    visibility = None,
):
    """
    Custom macro for buck2/buckaemon end-to-end tests.

    The test is a `python_unittest` on the Python harness (`e2e_util/buck_workspace.py`), or, if
    `srcs` are Rust, a `rust_unittest` on the Rust harness (`e2e_util/rust`) whose crate root is
    the first of `srcs`.
    """
    srcs = srcs or []
    is_rust = _is_rust_test(name, srcs)
    labels = labels or []
    deps = deps or []
    cfg_modifiers = cfg_modifiers or []
    if remote_execution == None and not re_test_utils.labels_have_re_signal(labels):
        remote_execution = False
    if network_access == None:
        network_access = network_access_utils.all()

    for s in skip_for_os:
        if s not in ["darwin", "windows"]:
            fail("Skipped os must be one of darwin or windows, not {}".format(s))
    labels = list(labels)
    env = env or {}
    env["RUST_BACKTRACE"] = "1"
    env["TEST_EXECUTABLE"] = executable

    # Having it enabled has significant negative impact on Starlark evaluation performance.
    env["RUST_LIB_BACKTRACE"] = "0"

    # For autodeps
    read_package_value = getattr(native, "read_package_value", None)
    e2e_flavor = read_package_value and read_package_value("buck2_e2e_test.flavor")
    if e2e_flavor == "isolated":
        env["BUCK2_E2E_TEST_FLAVOR"] = "isolated"
        serialize_test_cases = serialize_test_cases or False
    else:
        env["BUCK2_E2E_TEST_FLAVOR"] = "any"
        serialize_test_cases = serialize_test_cases if serialize_test_cases != None else True
    heavyweight_label = {
        "4": "heavyweight",
        "8": "heavyweight8_experimental",
    }.get(heavyweight_threads)
    if heavyweight_label == None:
        fail("Unsupported heavyweight thread count: {}".format(heavyweight_threads))

    # Running multiple bucks are expensive. This label specifies how many CPU slots each test gets
    # when TPX schedules them. See different possible values for heavyweight label here:
    # https://www.internalfb.com/wiki/TAE/tpx/Tpx_user_guide/#tests-that-oom-or-time-o.
    labels.append(heavyweight_label)

    # Use little threads. We don't do much work in tests but we do run lots of Bucks.
    env["BUCK2_RUNTIME_THREADS"] = heavyweight_threads
    env["BUCK2_MAX_BLOCKING_THREADS"] = heavyweight_threads

    if serialize_test_cases:
        # This lets us pass stress runs by making all test cases inside of a test file serial
        # Test cases in different files can still run in parallel.
        labels.append("serialize_test_cases")

    if data and data_dir:
        fail("`data` and `data_dir` cannot be used together")

    # If there's `_data` dir next to test, use it as test data directory.
    if data_dir:
        if not data_dir.startswith("test_") or not data_dir.endswith("_data"):
            fail("Data dirs must be of the form `test_.*_data`, not {}".format(data_dir))
        buck_filegroup(
            name = data_dir,
            srcs = [data_dir],
            copy = False,
        )
        env["TEST_REPO_DATA"] = "$(location :{d})/{d}".format(d = data_dir)

    if data:
        env["TEST_REPO_DATA"] = "$(location {})".format(data)

    # Add nano_prelude unconditionally for isolated tests
    if require_nano_prelude == None:
        require_nano_prelude = data_dir != None
    if require_nano_prelude:
        env["NANO_PRELUDE"] = "$(location fbcode//buck2/tests/e2e_util/nano_prelude:nano_prelude)"

    ci_removals = []
    if "darwin" in skip_for_os:
        ci_removals.append(ci.remove(ci.mac(ci.aarch64(ci.opt()))))
    if "windows" in skip_for_os:
        ci_removals.append(ci.remove(ci.windows(ci.opt())))
    if ci_removals:
        labels = _remove_ci_labels(labels, ci_removals)

    kwargs = {"visibility": visibility} if visibility != None else {}
    if is_rust:
        if base_module != None:
            fail("`base_module` does not apply to the Rust test {}".format(name))
        run_env = dict(env)

        # `run_env` gives the paths to outputs relative to the project root, where a Rust test
        # starts; the harness makes these absolute for the buck2 it runs elsewhere.
        path_vars = [key for key, value in env.items() if type(value) != "string" or "$(location" in value]
        if path_vars:
            run_env["BUCK2_E2E_PATH_VARS"] = ",".join(sorted(path_vars))

        # An inplace test runs buck2 in its cell's root, with an isolation dir named after the
        # test target.
        run_env["BUCK2_E2E_CELL"] = native.get_cell_name()
        run_env["BUCK2_E2E_TARGET"] = "{}//{}:{}".format(native.get_cell_name(), native.package_name(), name)

        # As with `python_unittest`'s `skip_on_mode_*`, a build mode the tests skip gets a
        # placeholder instead.
        if is_noop_rule("rust_unittest", skip_on_mode_mac = "darwin" in skip_for_os, skip_on_mode_win = "windows" in skip_for_os):
            xplat_shims.dummy_node(name = name, msg = "rust e2e test was skipped on this platform.")
            return

        # Elsewhere, the tests pass without running on these.
        if skip_for_os:
            run_env["BUCK2_E2E_SKIP_FOR_OS"] = ",".join(skip_for_os)
        rust_unittest(
            name = name,
            srcs = srcs,
            crate_root = srcs[0],
            deps = deps + ["fbcode//buck2/tests/e2e_util/rust:buck2_e2e"],
            # Unlike `run_env`, `env` would also set the variables for rustc.
            run_env = run_env,
            labels = labels,
            contacts = contacts,
            remote_execution = remote_execution,
            network_access = network_access,
            resources = resources,
            modifiers = cfg_modifiers,
            compatible_with = compatible_with,
            # The tests build with buck only.
            autocargo = {"ignore_rule": True},
            **kwargs,
        )
    else:
        deps += ["fbcode//buck2/tests/e2e_util:utilities"]
        if use_buck_api:
            deps += ["fbcode//buck2/tests/e2e_util/api:api"]
        python_unittest(
            name = name,
            base_module = base_module,
            srcs = srcs,
            labels = labels,
            deps = deps,
            env = env,
            emails = contacts,
            remote_execution = remote_execution,
            network_access = network_access,
            resources = resources,
            skip_on_mode_mac = "darwin" in skip_for_os,
            skip_on_mode_win = "windows" in skip_for_os,
            modifiers = cfg_modifiers,
            compatible_with = compatible_with,
            # Static listing reads the test names off the source: it would miss the test cases
            # `parametrize()` generates and list the method they come from.
            supports_static_listing = False,
            **kwargs,
        )

    if e2e_flavor == "buck2_non_isolated":
        # These are buck2's own non-isolated e2e tests. Add a ci hint indicating
        # that they depend on many of the macros in the repo. Intentionally
        # don't do this for other users of `buck2_e2e_test` in the repo
        BUCK2_E2E_TEST_CI_SRCS = [
            "fbandroid/buck2/**",
            "fbcode/buck2/cfg/**",
            "fbcode/buck2/prelude/**",
            "fbcode/buck2/platform/**",
            "fbcode/buck2/toolchains/**",
            "fbcode/buck2/tests/targets/**",
            "fbobjc/buck2/**",
            "xplat/buck2/**",
            "xplat/toolchains/**",
            "!xplat/toolchains/gator/**",
            "fbcode/hermetic_infra/fdb/**",
            "tools/build_defs/**",
            "!tools/build_defs/llvm/**",
            "arvr/tools/build_defs/config/**",
            ".buckconfig",
            "tools/buckconfigs/**",
        ]
        ci_srcs = ci_srcs + BUCK2_E2E_TEST_CI_SRCS
    if ci_srcs or ci_deps:
        ci_hint(
            ci_srcs = ci_srcs,
            ci_deps = ci_deps,
            reason = "Non isolated buck2 e2e tests depend heavily on macros",
            target = name,
            compatible_with = compatible_with,
            # This matches the actual test binary (via _convert_binary in
            # tools/build_defs/fbcode_macros/build_defs/lib/python_common.bzl).
            default_target_platform = platform_utils.get_cxx_platform_for_current_buildfile().target_platform,
        )

def _is_rust_test(name, srcs):
    rust = [src for src in srcs if src.endswith(".rs")]
    if rust and len(rust) != len(srcs):
        fail("{} mixes Rust sources with others".format(name))
    return len(rust) > 0

def _remove_ci_labels(labels, removals):
    """Apply CI removals without discarding CI labels the caller set.

    Only the last `ci:overwrite` group on a target counts, so the caller's CI
    labels must live in the same group as the removals.
    """
    ci_labels = [label for label in labels if label.startswith("ci:")]
    other_labels = [label for label in labels if not label.startswith("ci:")]
    if "ci:overwrite" in ci_labels:
        for remove in removals:
            ci_labels = remove(ci_labels)
        return other_labels + ci_labels
    return other_labels + ci.labels(*(ci_labels + removals))

def buck2_e2e_test(
    name,
    test_with_compiled_buck2 = True,
    test_with_deployed_buck2 = False,
    test_with_reverted_buck2 = False,
    use_compiled_buck2_client_and_tpx = False,
    skip_deployed_buck2_version_dep = False,
    deps = (),
    env = None,
    skip_for_os = (),
    use_buck_api = True,
    contacts = None,
    base_module = None,
    data = None,
    data_dir = None,
    srcs = (),
    labels = (),
    resources = None,
    serialize_test_cases = None,
    require_nano_prelude = None,
    ci_srcs = [],
    ci_deps = [],
    compatible_with = None,
    heavyweight_threads = "4",
    remote_execution = None,
    network_access = None,
    visibility = None,
):
    """
    Custom macro for buck2 end-to-end tests. All tests are run against buck2 compiled in-repo (compiled buck2).

    test_with_compiled_buck2:
        A boolean for whether to run tests with the compiled buck2.
        Default is True.
        Should typically be unset when testing things that are not expected to be disproportionately
        sensitive to buck2 core changes. Unsetting this also simplifies the CI setup, as testing
        with buck2 core requires always using opt mode.
    test_with_deployed_buck2:
        A boolean for whether to run tests with the deployed buck2.
        Default is False.
        Should typically be set for tests of UDRs and other things that are not "core buck2 functionality"
    test_with_reverted_buck2:
        Like `test_with_deployed_buck2`, but for the previous version
    use_compiled_buck2_client_and_tpx:
        A full prod archive is distinct from a normal build of buck2 in that it uses a client-only
        binary and additionally makes TPX available. Needed if you want to be able to `buck.test`
        Default is False.
    skip_deployed_buck2_version_dep:
        A boolean for whether to skip adding the dependency on tools/buck2-versions:stable when
        test_with_deployed_buck2 is True. This is useful for tests that don't need the version
        dependency for Target Determinator purposes (e.g., bxl tests that test Starlark logic).
        Default is False.
    """
    kwargs = {
        "base_module": base_module,
        "ci_deps": ci_deps,
        "ci_srcs": ci_srcs,
        "compatible_with": compatible_with,
        "contacts": contacts,
        "data": data,
        "data_dir": data_dir,
        "heavyweight_threads": heavyweight_threads,
        "labels": labels,
        "network_access": network_access,
        "remote_execution": remote_execution,
        "require_nano_prelude": require_nano_prelude,
        "resources": resources,
        "serialize_test_cases": serialize_test_cases,
        "srcs": srcs,
        "use_buck_api": use_buck_api,
        "visibility": visibility,
    }

    env = env or {}
    if not test_with_compiled_buck2 and not test_with_deployed_buck2:
        fail("Must set one of `test_with_compiled_buck2` or `test_with_deployed_buck2` for " + name)

    # soft errors should always be allowed on tests with deployed buck2, or with reverted buck
    deployed_env = dict(env)
    deployed_env["BUCK2_HARD_ERROR"] = "false"

    if test_with_compiled_buck2:
        compiled_env = dict(env)

        # TODO(ctolliday) use BUCK2_HARD_ERROR=panic
        compiled_env["BUCK2_HARD_ERROR"] = "true"
        compiled_env["BUCK2_TPX"] = "$BUCK2_BINARY_DIR/buck2-tpx"

        # The coverage wrapper maps profiles only against the test's own executable and
        # `LLVM_COVERAGE_ADDITIONAL_OBJECT_PATHS`. Each branch lists the binaries the test runs;
        # without them, their coverage is dropped and coverage-based test selection does not
        # link these tests to buck2 or tpx sources.
        if use_compiled_buck2_client_and_tpx:
            compiled_env["LLVM_COVERAGE_ADDITIONAL_OBJECT_PATHS"] = ";".join(
                ["$(location fbcode//buck2:symlinked_buck2_and_tpx)/" + binary for binary in ["buck2", "buck2-daemon", "buck2-tpx"]]
            )
            base_exe = "$(location fbcode//buck2:symlinked_buck2_and_tpx)/buck2"
            exe = select({
                "DEFAULT": base_exe,
                "ovr_config//os:windows": base_exe + ".exe",
            })
        else:
            compiled_env["LLVM_COVERAGE_ADDITIONAL_OBJECT_PATHS"] = "$(location fbcode//buck2:buck2)"
            exe = "$(location fbcode//buck2:buck2)"

        buck_e2e_test(
            # deployed buck2 test target retains the original target name so that when user runs `buck test <test target>`,
            # it only runs the deployed buck2 tests and not the compiled buck2 tests.
            # This will make it much quicker for rule writers to run their tests locally.
            name = name + ("_with_compiled_buck2" if test_with_deployed_buck2 else ""),
            env = compiled_env,
            executable = exe,
            skip_for_os = skip_for_os,
            deps = deps,
            cfg_modifiers = buck2_modifiers()
            + [
                # Always run these tests under rust opt build
                "ovr_config//build_mode:opt",
            ],
            **kwargs,
        )

    if test_with_deployed_buck2:
        deps = deps or []

        # Add a buck2 version file as dep so we can run deployed buck2 tests on version bumps.
        # Skip this dependency if skip_deployed_buck2_version_dep is True (e.g., for bxl tests).
        if not skip_deployed_buck2_version_dep:
            deps += ["fbsource//tools/buck2-versions:stable"]
        buck_e2e_test(name = name, env = deployed_env, executable = "buck2", skip_for_os = skip_for_os, deps = deps, **kwargs)

    if test_with_reverted_buck2:
        previous_env = dict(deployed_env)
        previous_env["BUCK2_CHANNEL"] = "previous"
        buck_e2e_test(name = name + "_with_reverted_buck2", env = previous_env, executable = "buck2", skip_for_os = skip_for_os, deps = deps, **kwargs)

# Materializer implementations other than the default, by the names `buck_workspace.py` maps to
# their configs.
OTHER_MATERIALIZERS = ["v2"]

def buck2_core_tests(extra_attrs = {}, target_extra_attrs = {}, allow_bundled_cells = False, materializer_variants = []):
    """
    A little wrapper that generates `buck2_e2e_test`s for core tests.

    extra_attrs:
        Extra attributes that are applied to all generated targets.
    target_extra_attrs:
        A map of target name to extra attrs to apply to that target.
    allow_bundled_cells:
        Lets fixtures use bundled external cells other than the nano prelude, which the test
        harness otherwise rejects. Only for tests of the external cells machinery itself.
    materializer_variants:
        Test targets to also generate as `<target>_<materializer>` for every other
        materializer implementation, so that a suite runs against each core.
    """

    # @lint-ignore BUCKRESTRICTEDSYNTAX
    items = set([
        i.split("/")[0]
        for i in glob(
            ["**/*", "**/.*"],
            exclude = [".pyre_configuration.local"],
        )
    ])
    items = list(items)

    generated_targets = []

    for item in items:
        if item in ["TARGETS", "TARGETS.v2", "BUCK", "BUCK.v2"]:
            continue
        if item.startswith("test_") and item.endswith("_data"):
            # Just make sure the associated test exists
            if item[:-5] + ".py" not in items and item[:-5] + ".rs" not in items:
                fail("Test data directory {} exists but has no matching test!".format(item))
            continue
        if item.startswith("test_") and (item.endswith(".py") or item.endswith(".rs")):
            target = item[:-3]
            generated_targets.append(target)

            attrs = dict(extra_attrs)
            attrs.update(target_extra_attrs.get(target) or {})
            if allow_bundled_cells:
                attrs["env"] = dict(attrs.get("env") or {})
                attrs["env"]["BUCK2_E2E_TEST_ALLOW_BUNDLED_CELLS"] = "1"

            data_dir = target + "_data"
            if data_dir not in items:
                data_dir = None
            if "data_dir" in attrs:
                fail("May not set data dir in `extra_attrs`")
            attrs["data_dir"] = data_dir

            if "srcs" not in attrs:
                # Allowing people to override `srcs` seems fine
                attrs["srcs"] = [item]

            attrs["deps"] = list(attrs.get("deps") or [])
            if item.endswith(".py"):
                attrs["deps"].extend([
                    "//buck2/tests/e2e_util:utils",
                    "//buck2/tests/e2e_util:golden",
                ])

            buck2_e2e_test(name = target, **attrs)
            if target in materializer_variants:
                for materializer in OTHER_MATERIALIZERS:
                    variant_attrs = dict(attrs)
                    variant_attrs["env"] = dict(attrs.get("env") or {})
                    variant_attrs["env"]["BUCK2_TEST_MATERIALIZER"] = materializer
                    if data_dir:
                        # The data filegroup already exists for the main target.
                        variant_attrs["data_dir"] = None
                        variant_attrs["env"]["TEST_REPO_DATA"] = "$(location :{d})/{d}".format(d = data_dir)
                        variant_attrs.setdefault("require_nano_prelude", True)
                    buck2_e2e_test(name = target + "_" + materializer, **variant_attrs)
            continue
        fail("Expected all directory entries to look like `test_*_data`, `test_*.py` or `test_*.rs`, not {}".format(item))

    # Check that all target attrs actually correspond to a target
    for t in target_extra_attrs.keys():
        if t not in generated_targets:
            fail("No such target {}".format(t))
    for t in materializer_variants:
        if t not in generated_targets:
            fail("No such target {}".format(t))
