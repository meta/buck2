# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//apple:apple_bundle_destination.bzl", "AppleBundleDestination", "bundle_relative_path_for_destination")
load("@prelude//apple:apple_bundle_types.bzl", "AppleBundleInfo", "ApplePackageExtension")
load("@prelude//apple:apple_common.bzl", "apple_common")
load("@prelude//apple:apple_package_config.bzl", "IpaCompressionLevel")
load("@prelude//apple:apple_rules_impl_utility.bzl", "get_apple_bundle_toolchain_attr")
load("@prelude//apple:apple_sdk.bzl", "get_apple_sdk_name")
load("@prelude//apple:apple_swift_stdlib.bzl", "should_copy_swift_stdlib")
load("@prelude//apple:debug.bzl", "AppleDebuggableInfo")
load("@prelude//apple/swift:swift_toolchain_types.bzl", "SwiftToolchainInfo")
load("@prelude//utils:arglike.bzl", "ArgLike")

# Produced by the `apple_app_symbols` target. It carries the App Store `<UUID>.symbols`
# directory and the Swift stdlib dir.
AppleAppSymbolsInfo = provider(
    fields = {
        "swift_support_dir": provider_field(Artifact | None, default = None),
        "symbols_dir": provider_field(Artifact),
    },
)

def apple_ipa_package_impl(ctx: AnalysisContext) -> list[Provider]:
    ipa_package = _get_ipa_contents(ctx)
    return [DefaultInfo(default_output = ipa_package)]

def apple_ipa_package_attribs():
    attribs = {
        "bundle": attrs.dep(providers = [AppleBundleInfo]),
        "ext": attrs.enum(ApplePackageExtension.values(), default = "ipa"),
        "include_app_symbols": attrs.bool(default = False),
        "labels": attrs.list(attrs.string(), default = []),
        "package_name": attrs.option(attrs.string(), default = None),
        "_app_symbols": attrs.option(attrs.dep(providers = [AppleAppSymbolsInfo]), default = None),
        "_apple_toolchain": get_apple_bundle_toolchain_attr(),
        "_ipa_compression_level": attrs.enum(IpaCompressionLevel.values()),
    }
    attribs.update(apple_common.apple_tools_arg())
    return attribs

_IPA_PACKAGE_FORWARDED_FIELDS = [
    "bundle",
    "ext",
    "include_app_symbols",
    "package_name",
    "_ipa_compression_level",
    "compatible_with",
    "exec_compatible_with",
    "target_compatible_with",
    "default_target_platform",
    "within_view",
    "visibility",
]

def make_apple_ipa_package_target(apple_ipa_package_rule, **kwargs) -> [None, str]:
    ipa_package_kwargs = {
        "labels": ["generated"],
    }
    for field_name in _IPA_PACKAGE_FORWARDED_FIELDS:
        ipa_package_kwargs[field_name] = kwargs.get(field_name)

    # Only define the symbols target when app symbols are requested, either when set to
    # True or we have a select to evaluate.
    if kwargs.get("include_app_symbols"):
        app_symbols_target_name = kwargs["name"] + "__AppSymbols_Private"
        apple_app_symbols_rule(
            name = app_symbols_target_name,
            bundle = kwargs.get("bundle"),
            include_app_symbols = kwargs.get("include_app_symbols"),
            labels = ["generated"],
            exec_compatible_with = ["config//os:macos"],
            compatible_with = kwargs.get("compatible_with"),
            target_compatible_with = kwargs.get("target_compatible_with"),
            default_target_platform = kwargs.get("default_target_platform"),
            within_view = kwargs.get("within_view"),
            visibility = kwargs.get("visibility"),
        )
        ipa_package_kwargs["_app_symbols"] = ":{}".format(app_symbols_target_name)

    ipa_package_target_name = kwargs["name"] + "__IPA_Package_Private"
    apple_ipa_package_rule(name = ipa_package_target_name, **ipa_package_kwargs)

    return ":{}".format(ipa_package_target_name)

def _get_ipa_contents(ctx: AnalysisContext) -> Artifact:
    bundle = ctx.attrs.bundle
    app = bundle[DefaultInfo].default_outputs[0]

    contents = {
        paths.join("Payload", app.basename): app,
    }

    apple_bundle_info = bundle[AppleBundleInfo]
    swift_needed = (not apple_bundle_info.skip_copying_swift_stdlib) and should_copy_swift_stdlib(app.extension)

    app_symbols_info = ctx.attrs._app_symbols[AppleAppSymbolsInfo] if ctx.attrs._app_symbols else None
    if ctx.attrs.include_app_symbols and app_symbols_info != None:
        # App symbols requested: the macOS-only apple_app_symbols target already
        # produced the Swift stdlib dir (computed once, there) and the populated
        # Symbols/ dir. Copy both in; no Mac-only action runs in this rule.
        if swift_needed:
            contents[paths.join("SwiftSupport", get_apple_sdk_name(ctx))] = app_symbols_info.swift_support_dir
        contents["Symbols"] = app_symbols_info.symbols_dir
    else:
        if swift_needed:
            contents[paths.join("SwiftSupport", get_apple_sdk_name(ctx))] = _get_swift_support_dir(ctx, app, apple_bundle_info)

        # watchOS IPAs require a top-level (possibly empty) Symbols/ dir to exist.
        if apple_bundle_info.contains_watchapp:
            contents["Symbols"] = _empty_symbols_dir(ctx)

    return ctx.actions.copied_dir(
        "__unzipped_ipa_contents__",
        contents,
        has_content_based_path = False,
    )

def _empty_symbols_dir(ctx) -> Artifact:
    symbols_dir = ctx.actions.declare_output("__symbols__", dir = True, has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["mkdir", "-p", symbols_dir.as_output()]),
        category = "watchos_symbols_dir",
    )
    return symbols_dir

def _apple_app_symbols_impl(ctx: AnalysisContext) -> list[Provider]:
    bundle = ctx.attrs.bundle
    app = bundle[DefaultInfo].default_outputs[0]
    apple_bundle_info = bundle[AppleBundleInfo]

    # When include_app_symbols resolves False for this configuration, produce an
    # empty dir (unconsumed by the IPA package, so its action never runs) and skip
    # the Swift stdlib work — the IPA package computes Swift support itself in that
    # path, so the stdlib tool is still only ever run once.
    if not ctx.attrs.include_app_symbols:
        symbols_dir = _empty_symbols_dir(ctx)
        return [
            DefaultInfo(default_output = symbols_dir),
            AppleAppSymbolsInfo(symbols_dir = symbols_dir),
        ]

    swift_support_dir = None
    if (not apple_bundle_info.skip_copying_swift_stdlib) and should_copy_swift_stdlib(app.extension):
        swift_support_dir = _get_swift_support_dir(ctx, app, apple_bundle_info)

    symbols_dir = _build_app_symbols_dir(ctx, swift_support_dir)
    return [
        DefaultInfo(default_output = symbols_dir),
        AppleAppSymbolsInfo(
            symbols_dir = symbols_dir,
            swift_support_dir = swift_support_dir,
        ),
    ]

def _build_app_symbols_dir(ctx, swift_support_dir) -> Artifact:
    # Emit one <UUID>.symbols per Mach-O slice from the bundle's dSYMs (and bundled
    # Swift stdlib dylibs).
    symbols_dir = ctx.actions.declare_output("__symbols__", dir = True, has_content_based_path = False)
    dsyms = ctx.attrs.bundle[AppleDebuggableInfo].dsyms

    script_lines = [
        cmd_args("set -euo pipefail"),
        cmd_args(symbols_dir, format = "mkdir -p {}"),
    ]
    symbol_inputs = []
    if dsyms:
        script_lines.append(
            cmd_args(
                ["symbols", "-arch", "all", "-symbolsPackageDir", symbols_dir] + dsyms,
                delimiter = " ",
                quote = "shell",
            )
        )
        symbol_inputs.extend(dsyms)
    if swift_support_dir != None:
        script_lines.append(
            cmd_args(
                [
                    "symbols",
                    "-arch",
                    "all",
                    "-symbolsPackageDir",
                    symbols_dir,
                    cmd_args(swift_support_dir, format = "{}/*"),
                    "||",
                    "echo",
                    "'warning: swift stdlib symbols packaging failed'",
                    ">&2",
                ],
                delimiter = " ",
                quote = "shell",
            )
        )
        symbol_inputs.append(swift_support_dir)

    script, _ = ctx.actions.write(
        "build_app_symbols.sh",
        script_lines,
        allow_args = True,
        has_content_based_path = False,
    )
    ctx.actions.run(
        cmd_args(["/bin/sh", script], hidden = symbol_inputs + [symbols_dir.as_output()]),
        category = "app_symbols_dir",
    )

    return symbols_dir

apple_app_symbols_rule = rule(
    impl = _apple_app_symbols_impl,
    attrs = {
        "bundle": attrs.dep(providers = [AppleBundleInfo, AppleDebuggableInfo]),
        "include_app_symbols": attrs.bool(default = False),
        "labels": attrs.list(attrs.string(), default = []),
        "_apple_toolchain": get_apple_bundle_toolchain_attr(),
    },
)

def _get_swift_support_dir(ctx, bundle_output: Artifact, bundle_info: AppleBundleInfo) -> Artifact:
    stdlib_tool = ctx.attrs._apple_toolchain[SwiftToolchainInfo].swift_stdlib_tool
    sdk_name = get_apple_sdk_name(ctx)

    # .app -> app
    # This is the way the input is expected.
    extension = bundle_output.extension[1:]
    swift_support_dir = ctx.actions.declare_output("__swift_dylibs__", dir = True, has_content_based_path = False)
    script, _ = ctx.actions.write(
        "build_swift_support.sh",
        [
            cmd_args("set -euo pipefail"),
            cmd_args(swift_support_dir, format = "mkdir -p {}"),
            cmd_args(
                [
                    stdlib_tool,
                    # If you're debugging, you can pass the '--verbose' flag here.
                    "--copy",
                    "--scan-executable",
                    cmd_args(
                        [
                            bundle_output,
                            bundle_relative_path_for_destination(AppleBundleDestination("executables"), sdk_name, extension, False),
                            bundle_info.binary_name,
                        ],
                        delimiter = "/",
                    ),
                    _get_scan_folder_args(AppleBundleDestination("plugins"), bundle_output, sdk_name, extension),
                    _get_scan_folder_args(AppleBundleDestination("extensionkit_extensions"), bundle_output, sdk_name, extension),
                    _get_scan_folder_args(AppleBundleDestination("frameworks"), bundle_output, sdk_name, extension),
                    _get_scan_folder_args(AppleBundleDestination("appclips"), bundle_output, sdk_name, extension),
                    "--destination",
                    swift_support_dir,
                ],
                delimiter = " ",
                quote = "shell",
            ),
        ],
        allow_args = True,
        has_content_based_path = False,
    )
    ctx.actions.run(
        cmd_args(["/bin/sh", script], hidden = [stdlib_tool, bundle_output, swift_support_dir.as_output()]),
        category = "copy_swift_stdlibs",
    )

    return swift_support_dir

def _get_scan_folder_args(dest: AppleBundleDestination, bundle_output: Artifact, sdk_name, extension) -> ArgLike:
    return cmd_args(
        [
            "--scan-folder",
            cmd_args(
                [
                    bundle_output,
                    bundle_relative_path_for_destination(dest, sdk_name, extension, False),
                ],
                delimiter = "/",
            ),
        ],
    )
