# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused Used as type
load(":attrs.bzl", "erlang_rule_attributes")
load(":erlang_application.bzl", "StartDependencySet", "StartSpec", "build_apps_start_dependencies")
load(":erlang_build.bzl", "erlang_build")
load(":erlang_dependencies.bzl", "ErlAppDependencies", "flatten_dependencies")
load(":erlang_info.bzl", "ErlangAppInfo", "ErlangReleaseInfo", "StartType")
load(
    ":erlang_toolchain.bzl",
    "Toolchain",  # @unused Used as type
    "get_toolchain",
)

# Erlang Releases according to https://www.erlang.org/doc/design_principles/release_structure.html

Release = record(
    dir = Artifact,
    # `bin/<name>` on its own, so a release that installs another's launcher does not take its tree
    launcher = field(Artifact | None, None),
    # what the release is assembled from that analysis already knows, by path from the release root
    entries = dict[str, Artifact],
)

LauncherLines = record(
    name = str,
    head = list[str],
    tail = list[str],
)

ReleaseConfig = record(
    # the target the release is built for, only used to point failures at it
    label = Label,
    name = str,
    version = str,
    # the applications the release starts, in order, with the type each is started with, and
    # together with their transitive dependencies what `lib/` is drawn from
    applications = list[(Dependency, StartType)],
    toolchain = Toolchain,
    # the environment every toolchain invocation runs with, `None` for the toolchain's own
    os_env = field(dict[str, str] | None, None),
    include_erts = bool,
    is_executable = bool,
    generate_default_bootscript = bool,
    default_bootscript_name = str,
    bootscript_builders = dict[str, cmd_args],
    extra_bootscript_builder_args = list[ArgLike],
    # the config files the launcher hands the emulator, in the order they are applied, so a later
    # one overrides an earlier one. Each is a path from the release root without the `.config`
    # extension, the form `-config` names a file with, and has to be part of the release.
    config_paths = list[str],
    # artifacts to install, mapping the directory they go into, from the release root, to their contents
    overlays = dict[str, list[Artifact]],
    # artifacts to install, by their path from the release root, that the release does not build itself
    extra_entries = field(dict[str, Artifact], {}),
)

def erlang_release_impl(ctx: AnalysisContext) -> list[Provider]:
    config = _release_config(ctx)

    release = build_release(ctx.actions, config)
    providers = [DefaultInfo(default_output = release.dir), ErlangReleaseInfo(name = config.name)]

    if config.is_executable:
        # the launcher reaches the rest of the release through relative symlinks, so running it
        # needs the whole tree materialised, not just `bin/<name>`
        launcher = release.dir.project(_launcher_path(config)).with_associated_artifacts([release.dir])
        providers.append(RunInfo(cmd_args(launcher)))

    return providers

def _release_config(ctx: AnalysisContext) -> ReleaseConfig:
    applications = _applications(ctx)
    toolchain = get_toolchain(ctx)
    overlays = {
        target: [artifact for dep in deps for artifact in dep[DefaultInfo].default_outputs + dep[DefaultInfo].other_outputs]
        for target, deps in ctx.attrs.overlays.items()
    }

    # an `erlang_release` is configured with the one `sys.config` the OTP layout gives it
    sys_config = paths.join("releases", ctx.attrs.version, "sys")
    config_paths = [sys_config] if sys_config + ".config" in _overlay_paths(overlays) else []

    return ReleaseConfig(
        label = ctx.label,
        name = ctx.attrs.release_name if ctx.attrs.release_name else ctx.attrs.name,
        version = ctx.attrs.version,
        applications = applications,
        toolchain = toolchain,
        os_env = getattr(ctx.attrs, "os_env", None),
        include_erts = ctx.attrs.include_erts,
        is_executable = ctx.attrs.is_executable,
        generate_default_bootscript = ctx.attrs.generate_default_bootscript,
        default_bootscript_name = ctx.attrs.default_bootscript_name,
        bootscript_builders = {script_name: builder[RunInfo].args for script_name, builder in ctx.attrs.bootscript_builders.items()},
        extra_bootscript_builder_args = ctx.attrs.extra_bootscript_builder_args,
        config_paths = config_paths,
        overlays = overlays,
    )

def _applications(ctx: AnalysisContext) -> list[(Dependency, StartType)]:
    """Extract the applications, with their start type, from the `applications` field, order preserving"""
    applications = []
    for dep in ctx.attrs.applications:
        if type(dep) == "tuple":
            applications.append((dep[0], StartType(dep[1])))
        else:
            applications.append((dep, StartType("permanent")))
    return applications

def build_release(actions: AnalysisActions, config: ReleaseConfig) -> Release:
    """Build an OTP release, returning the release root.

    The versioned parts of the layout, `erts-<version>` and `lib/<application>-<version>`, are laid
    out by a dynamic action: the versions come from the toolchain's OTP rather than from analysis.
    """
    _validate_include_erts(config)
    _validate_is_executable(config)

    all_apps = flatten_dependencies([app for app, _ in config.applications])
    own_apps = _own_applications(all_apps)
    otp_apps = _otp_applications(config, all_apps) if config.include_erts else {}
    erts_toolchain_info = config.toolchain.erts_toolchain_info

    if erts_toolchain_info == None:
        lib_dir = actions.symlinked_dir(paths.join(erlang_build.utils.BUILD_DIR, "lib"), own_apps, has_content_based_path = False)
    else:
        lib_dir = actions.declare_output(paths.join(erlang_build.utils.BUILD_DIR, "lib"), dir = True, has_content_based_path = False)
        actions.dynamic_output_new(
            _assemble_lib_dir(
                otp_apps = otp_apps,
                out = lib_dir.as_output(),
                own_apps = own_apps,
                versions = erts_toolchain_info.versions,
            )
        )

    entries = {"lib": lib_dir}
    entries.update(_build_boot_scripts(actions, config, lib_dir))
    entries.update(_build_overlays(config.overlays))
    entries.update(_build_release_variables(actions, config))

    for entry, artifact in config.extra_entries.items():
        if entry in entries:
            fail("%s is given `%s` to install, which its own release builds" % (str(config.label), entry))
        entries[entry] = artifact

    launcher = _build_launcher(config, entries)
    if launcher != None and _launcher_path(config) in entries:
        fail("the launcher of %s is installed at %s, which the release already contains" % (str(config.label), _launcher_path(config)))

    launcher_file = None
    if launcher != None:
        launcher_file = actions.declare_output(paths.join(erlang_build.utils.BUILD_DIR, "launcher", launcher.name), has_content_based_path = False)

    if erts_toolchain_info == None:
        release_dir = actions.symlinked_dir(config.name, entries, has_content_based_path = False)
    else:
        release_dir = actions.declare_output(config.name, dir = True, has_content_based_path = False)
        actions.dynamic_output_new(
            _assemble_release(
                entries = entries,
                include_erts = config.include_erts,
                launcher = launcher,
                launcher_out = launcher_file.as_output() if launcher_file != None else None,
                otp_erts = erts_toolchain_info.erts,
                out = release_dir.as_output(),
                version = config.version,
                versions = erts_toolchain_info.versions,
            ),
        )
    return Release(dir = release_dir, entries = entries, launcher = launcher_file)

def _assemble_lib_dir_impl(
    actions: AnalysisActions, otp_apps: dict[str, Artifact], own_apps: dict[str, Artifact], versions: ArtifactValue, out: OutputArtifact
) -> list[Provider]:
    srcs = dict(own_apps)
    if otp_apps:
        app_versions = versions.read_json()["applications"]
        for app, app_folder in otp_apps.items():
            if app not in app_versions:
                fail("the toolchain's OTP does not contain the application `%s`" % (app,))
            srcs["{}-{}".format(app, app_versions[app])] = app_folder
    actions.symlinked_dir(out, srcs)
    return []

_assemble_lib_dir = dynamic_actions(
    impl = _assemble_lib_dir_impl,
    attrs = {
        "otp_apps": dynattrs.value(dict[str, Artifact]),
        "out": dynattrs.output(),
        "own_apps": dynattrs.value(dict[str, Artifact]),
        "versions": dynattrs.artifact_value(),
    },
)

def _assemble_release_impl(
    actions: AnalysisActions,
    entries: dict[str, Artifact],
    include_erts: bool,
    launcher: LauncherLines | None,
    launcher_out: OutputArtifact | None,
    otp_erts: Artifact,
    version: str,
    versions: ArtifactValue,
    out: OutputArtifact,
) -> list[Provider]:
    otp = versions.read_json()
    erts_dir = "erts-{}".format(otp["erts_version"])

    srcs = dict(entries)
    if include_erts:
        srcs[erts_dir] = otp_erts.project(erts_dir)

        start_erl_data = actions.declare_output("start_erl.data", has_content_based_path = False)
        actions.write(start_erl_data, "{} {}\n".format(otp["erts_version"], version))
        srcs[paths.join("releases", "start_erl.data")] = start_erl_data

    if launcher != None:
        lines = launcher.head + ['BINDIR="$ROOTDIR/{}/bin"'.format(erts_dir)] + launcher.tail
        srcs[paths.join("bin", launcher.name)] = actions.write(launcher_out, lines, is_executable = True)

    actions.symlinked_dir(out, srcs)
    return []

_assemble_release = dynamic_actions(
    impl = _assemble_release_impl,
    attrs = {
        "entries": dynattrs.value(dict[str, Artifact]),
        "include_erts": dynattrs.value(bool),
        "launcher": dynattrs.value(LauncherLines | None),
        "launcher_out": dynattrs.option(dynattrs.output()),
        "otp_erts": dynattrs.value(Artifact),
        "out": dynattrs.output(),
        "version": dynattrs.value(str),
        "versions": dynattrs.artifact_value(),
    },
)

def build_lib_dir(ctx: AnalysisContext, all_apps: ErlAppDependencies) -> dict[str, Artifact]:
    """Build lib dir according to OTP specifications.

    .. seealso:: `OTP Design Principles Release Structure <https://www.erlang.org/doc/design_principles/release_structure.html>`_
    """
    lib_dir = ctx.actions.symlinked_dir(
        paths.join(erlang_build.utils.BUILD_DIR, "lib"),
        _own_applications(all_apps),
        has_content_based_path = False,
    )
    return {"lib": lib_dir}

def _otp_applications(config: ReleaseConfig, all_apps: ErlAppDependencies) -> dict[str, Artifact]:
    applications = {
        "erts": config.toolchain.erts_toolchain_info.applications["erts"],
    }
    for dep in all_apps.values():
        if ErlangAppInfo not in dep or not dep[ErlangAppInfo].virtual:
            continue
        app_info = dep[ErlangAppInfo]
        if app_info.app_folder == None:
            fail("%s needs the OTP application `%s`, which the toolchain's OTP does not ship" % (str(config.label), app_info.name))
        applications[app_info.name] = app_info.app_folder
    return applications

def _own_applications(all_apps: ErlAppDependencies) -> dict[str, Artifact]:
    return {
        (dep[ErlangAppInfo].name + "-" + dep[ErlangAppInfo].version): dep[ErlangAppInfo].app_folder
        for dep in all_apps.values()
        if ErlangAppInfo in dep and not dep[ErlangAppInfo].virtual
    }

def _build_boot_scripts(actions: AnalysisActions, config: ReleaseConfig, lib_dir: Artifact) -> dict[str, Artifact]:
    link_spec = {}

    if config.generate_default_bootscript:
        maybe_default_boot_script = _build_default_boot_scripts(actions, config, lib_dir)
        link_spec.update(maybe_default_boot_script)

    # write applications spec to file
    data = [(app[ErlangAppInfo].name, start_type.value) for app, start_type in config.applications]
    spec_file = actions.write_json(
        paths.join(erlang_build.utils.BUILD_DIR, "bootscripts", "applications_json"),
        data,
        has_content_based_path = False,
    )

    for script_name, builder in config.bootscript_builders.items():
        custom_boot_script_spec = _build_custom_boot_scripts(actions, config, spec_file, script_name, builder, lib_dir)
        link_spec.update(custom_boot_script_spec)

    return link_spec

def _build_default_boot_scripts(actions: AnalysisActions, config: ReleaseConfig, lib_dir: Artifact) -> dict[str, Artifact]:
    """Build Name.rel, start.script, and start.boot in the release folder.

    Boot scripts are always generated regardless of include_erts setting.
    When include_erts=False (default), OTP applications use runtime version discovery.
    When include_erts=True, explicit versions from the toolchain are used and additional
    no_dot_erlang boot scripts are generated for the self-contained release.
    """
    release_name = config.name

    root_apps_names = [app[ErlangAppInfo].name for app, _ in config.applications]
    start_dependencies = build_apps_start_dependencies(actions, config.applications)

    root_set = actions.tset(
        StartDependencySet,
        value = StartSpec(
            name = "__ignored__",
            version = config.version,
            start_type = StartType("permanent"),
            resolved = False,
        ),
        children = start_dependencies,
    )

    reverse_start_order = list(root_set.traverse())
    reverse_start_order.pop(0)

    seen = set()
    release_applications = []
    root_apps_spec = {}
    for spec in reverse_start_order[::-1]:
        if spec.name in seen:
            continue
        seen.add(spec.name)

        app_spec = {
            "name": spec.name,
            "resolved": spec.resolved,
            "type": spec.start_type.value,
            "version": spec.version,
        }

        if spec.name in root_apps_names:
            root_apps_spec[spec.name] = app_spec
        else:
            release_applications.append(app_spec)
    for app_name in root_apps_names[::-1]:
        release_applications.append(root_apps_spec[app_name])

    data = {
        "apps": release_applications[::-1],
        "lib_dir": lib_dir,
        "name": release_name,
        "version": config.version,
    }

    spec_file = actions.write_json(paths.join(erlang_build.utils.BUILD_DIR, "boot_script_spec.json"), data, with_inputs = True, has_content_based_path = False)

    scripts_dir = actions.declare_output(erlang_build.utils.BUILD_DIR, "scripts", dir = True, has_content_based_path = False)

    _run_with_env(
        actions,
        config,
        cmd_args(config.toolchain.boot_script_builder, spec_file, scripts_dir.as_output()),
        category = "build_boot_script",
        identifier = release_name,
    )

    # Always include the standard boot scripts
    boot_files = [
        "{}.rel".format(release_name),
        "start.script",
        "start.boot",
    ]

    # Only include no_dot_erlang boot scripts for self-contained releases with bundled ERTS
    if config.include_erts:
        boot_files.extend([
            "no_dot_erlang.script",
            "no_dot_erlang.boot",
        ])

    result = {paths.join("releases", config.version, file): scripts_dir.project(file) for file in boot_files}

    # Place OTP's boot files in bin/ so erl can find them at ROOTDIR/bin/.
    # When erl runs from bundled ERTS (erts-VSN/bin/erl), it resolves ROOTDIR
    # to the release root and looks for bin/<name>.boot for boot files.
    # These are extracted once per toolchain (not per release) and contain only
    # kernel+stdlib, so that:
    #   - `erl` bare gives a clean shell (uses bin/start.boot)
    #   - `erl -boot no_dot_erlang` works for ectl and other tools
    # mini_start explicitly uses releases/VERSION/start.boot for service startup.
    if config.include_erts:
        result[paths.join("bin", "start.boot")] = config.toolchain.erts_toolchain_info.otp_start_boot
        result[paths.join("bin", "no_dot_erlang.boot")] = config.toolchain.erts_toolchain_info.otp_no_dot_erlang_boot

    return result

def _build_custom_boot_scripts(
    actions: AnalysisActions, config: ReleaseConfig, spec_file: Artifact, script_name: str, builder: cmd_args, lib_dir: Artifact
) -> dict[str, Artifact]:
    boot_script = actions.declare_output(paths.join(erlang_build.utils.BUILD_DIR, "bootscripts", script_name), has_content_based_path = False)
    raw_script_name = paths.replace_extension(script_name, ".script")
    raw_script = actions.declare_output(paths.join(erlang_build.utils.BUILD_DIR, "bootscripts", raw_script_name), has_content_based_path = False)

    _run_with_env(
        actions,
        config,
        cmd_args(
            builder,
            spec_file,
            lib_dir,
            boot_script.as_output(),
            raw_script.as_output(),
            config.extra_bootscript_builder_args,
        ),
        category = "build_custom_boot_script",
        identifier = script_name,
    )

    return {
        paths.join("releases", config.version, script_name): boot_script,
        paths.join("releases", config.version, raw_script_name): raw_script,
    }

def _build_overlays(overlays: dict[str, list[Artifact]]) -> dict[str, Artifact]:
    installed = {}
    for target, artifacts in overlays.items():
        for artifact in artifacts:
            link_path = _overlay_path(target, artifact)
            if link_path in installed:
                fail("multiple overlays defined for the same location: %s" % (link_path,))
            installed[link_path] = artifact
    return installed

def _overlay_paths(overlays: dict[str, list[Artifact]]) -> list[str]:
    return [_overlay_path(target, artifact) for target, artifacts in overlays.items() for artifact in artifacts]

def _overlay_path(target: str, artifact: Artifact) -> str:
    return paths.normalize(paths.join(target, artifact.basename))

def _build_release_variables(actions: AnalysisActions, config: ReleaseConfig) -> dict[str, Artifact]:
    release_name = config.name

    short_path = "bin/release_variables"
    release_variables = actions.declare_output(
        erlang_build.utils.BUILD_DIR,
        "release_variables",
        has_content_based_path = False,
    )

    spec_file = actions.write_json(
        paths.join(erlang_build.utils.BUILD_DIR, "relvars.json"),
        {
            "REL_NAME": release_name,
            "REL_VSN": config.version,
        },
        has_content_based_path = False,
    )

    _run_with_env(
        actions,
        config,
        cmd_args(config.toolchain.release_variables_builder, spec_file, release_variables.as_output()),
        category = "build_release_variables",
        identifier = release_name,
    )
    return {short_path: release_variables}

def _build_launcher(config: ReleaseConfig, release_files: dict[str, Artifact]) -> LauncherLines | None:
    """Generate bin/<release_name>, a launcher booting the release with the bundled emulator.

    Everything the emulator is told is resolved here rather than at runtime: the erts version, so
    the launcher addresses `erts-<version>` directly, and the boot script, `vm.args` and the config
    files, so the launcher does not depend on what it was invoked as. Every path is relative to
    ROOTDIR, so the release stays relocatable. The tool name is still taken from the launcher's own
    basename, so one release can serve several tools that differ only in the arguments they get.
    """
    if not config.is_executable:
        return None

    boot_script = paths.join("releases", config.version, config.default_bootscript_name)
    vm_args = boot_script + ".vm.args"

    for config_path in config.config_paths:
        if config_path + ".config" not in release_files:
            fail("%s is configured with `%s.config`, which none of its overlays installs" % (str(config.label), config_path))

    if boot_script + ".boot" not in release_files:
        fail("%s boots with `%s.boot`, which the release does not contain" % (str(config.label), boot_script))

    head = [
        "#!/usr/bin/env bash",
        "set -euo pipefail",
        # macOS has no `readlink -f`, so the symlink chain is followed one hop at a time
        'SELF="${BASH_SOURCE[0]}"',
        "HOPS=0",
        "while :; do",
        '    ROOTDIR="$(cd -P "$(dirname "$SELF")/.." && pwd)"',
        '    if [ -e "$ROOTDIR/{}.boot" ]; then'.format(boot_script),
        "        break",
        "    fi",
        '    if [ ! -L "$SELF" ]; then',
        '        echo "$0: cannot find the release root above $SELF" >&2',
        "        exit 1",
        "    fi",
        "    HOPS=$((HOPS + 1))",
        "    if [ $HOPS -gt 40 ]; then",
        '        echo "$0: too many symlink hops resolving release root" >&2',
        "        exit 1",
        "    fi",
        '    SELFDIR="$(cd -P "$(dirname "$SELF")" && pwd)"',
        '    SELF="$(readlink "$SELF")"',
        '    case "$SELF" in',
        "        /*) ;;",
        '        *) SELF="$SELFDIR/$SELF" ;;',
        "    esac",
        "done",
    ]
    tail = [
        'TOOL="$(basename "$0")"',
        "export ROOTDIR BINDIR",
        'exec "$BINDIR/erlexec" \\',
        '    -boot "$ROOTDIR/{}" \\'.format(boot_script),
    ]
    if vm_args in release_files:
        tail.append('    -args_file "$ROOTDIR/{}" \\'.format(vm_args))
    for config_path in config.config_paths:
        tail.append('    -config "$ROOTDIR/{}" \\'.format(config_path))
    tail += [
        '    -extra "$TOOL" ${1+"$@"}',
        "",
    ]

    return LauncherLines(name = config.name, head = head, tail = tail)

def _launcher_path(config: ReleaseConfig) -> str:
    return paths.join("bin", config.name)

def _run_with_env(actions: AnalysisActions, config: ReleaseConfig, args: cmd_args, **kwargs):
    """run interface that injects the environment the release's toolchain invocations run with"""
    env = config.os_env if config.os_env != None else config.toolchain.env

    if "env" in kwargs:
        kwargs["env"].update(env)
    else:
        kwargs["env"] = env

    actions.run(args, **kwargs)

def _validate_is_executable(config: ReleaseConfig) -> None:
    """Validate that a runnable release ships the emulator its launcher runs"""
    if config.is_executable and not config.include_erts:
        fail("is_executable = True requires include_erts = True, the launcher runs the emulator from the release's own erts folder: %s" % (str(config.label),))

def _validate_include_erts(config: ReleaseConfig) -> None:
    """Validate that a release bundling the emulator has a toolchain to take it from"""
    if config.include_erts and config.toolchain.erts_toolchain_info == None:
        fail(
            "include_erts = True requires the toolchain `%s` to set erts_toolchain_info, there is no ERTS nor OTP applications to take from it otherwise: %s"
            % (config.toolchain.name, str(config.label))
        )

erlang_release = prelude_rule(
    name = "erlang_release",
    impl = erlang_release_impl,
    docs = """
        The `erlang_release` target builds OTP releases. Please refer to the
        [OTP documentation](https://www.erlang.org/doc/design_principles/release_structure.html) for more details about
        releases.

        The `erlang_release` target does by default (without overlays) package:
        - applications that are required to start the release
        - release resource file `<relname>.rel` (see [rel(4)](https://www.erlang.org/doc/man/rel.html))
        - boot script `start.script` (see [rel(4)](https://www.erlang.org/doc/man/script.html))
        - binary boot script `start.boot`
        - `bin/release_variables`

        The `release_variables` file contains release name, version, and erts version in shell syntax, e.g.
        ```
        ERTS_VSN="12.1.2"
        REL_NAME="rel1"
        REL_VSN="1.0.0"
        ```

        The target name doubles as the default release name. If the `release_name` attribute is used, the release name will be
        sources from there instead.
    """,
    examples = """
        ```
        erlang_release(
            name = "world",
            version = "1.0.0",
            applications = [
                "//apps//app_a:app_a",
                "//apps//app_b:app_b",
            ],
            overlays = {
                "releases/1.0.0": [
                    ":sys.config.src",
                ],
                "bin": [
                    ":start.sh",
                ],
            },
        )

        export_file(
            name = "sys.config.src",
            src = "sys.config",
        )

        export_file(
            name = "start.sh",
            src = "start.sh",
        )
        ```
    """,
    further = None,
    attrs = erlang_rule_attributes["erlang_release"],
)
