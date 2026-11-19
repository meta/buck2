# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(":cxx_library_utility.bzl", "EMPTY_DEFAULT_INFO")
load(":cxx_toolchain_types.bzl", "CxxToolchainInfo", "LinkerInfo")

def shared_library_interface_path(shared_lib_short_path: str) -> str:
    """
    Where the interface derived from a shared library with the given short
    path lives, relative to the rule's output directory.
    """
    return paths.join("__shlib_intfs__", shared_lib_short_path)

def shared_library_interface_producer(linker_info: LinkerInfo) -> RunInfo:
    """
    The toolchain tool invoked as `<producer> <shared library> <interface>`.
    """
    if linker_info.mk_shlib_intf == None:
        fail(
            'Shared library interface generation is enabled (shlib_interfaces != "disabled") '
            + "but the toolchain does not define `shared_library_interface_producer`. "
            + "Either set `shared_library_interface_producer` in the toolchain or "
            + 'disable shared library interfaces by setting `shlib_interfaces = "disabled"`.',
        )
    return linker_info.mk_shlib_intf[RunInfo]

def run_shared_library_interface_producer(
    ctx: AnalysisContext, linker_info: LinkerInfo, shared_lib: [Artifact, Promise], interface: Artifact, identifier: [str, None]
):
    """
    Derive `interface` from `shared_lib` in a standalone action.
    """
    ctx.actions.run(
        cmd_args(shared_library_interface_producer(linker_info), shared_lib, interface.as_output()),
        category = "generate_shared_library_interface",
        identifier = identifier,
    )

def _shared_library_interface(ctx: AnalysisContext, output: str, identifier: str, shared_lib: [Artifact, Promise]) -> Artifact:
    """
    Convert the given shared library into an interface used for linking.
    """
    output = ctx.actions.declare_output(output, has_content_based_path = True)
    run_shared_library_interface_producer(
        ctx = ctx,
        linker_info = get_cxx_toolchain_info(ctx).linker_info,
        shared_lib = shared_lib,
        interface = output,
        identifier = identifier,
    )
    return output

_InterfaceInfo = provider(
    fields = {
        "artifact": provider_field(typing.Any, default = None),  # "artifact"
    }
)

def _anon_shared_library_interface_impl(ctx):
    output = _shared_library_interface(
        ctx = ctx,
        output = ctx.attrs.output,
        shared_lib = ctx.attrs.shared_lib,
        identifier = ctx.attrs.identifier,
    )
    return [EMPTY_DEFAULT_INFO, _InterfaceInfo(artifact = output)]

# Anonymous wrapper for `extract_symbol_names`.
_anon_shared_library_interface = anon_rule(
    impl = _anon_shared_library_interface_impl,
    attrs = {
        "identifier": attrs.option(attrs.string(), default = None),
        "output": attrs.string(),
        "shared_lib": attrs.source(),
        "_cxx_toolchain": attrs.dep(providers = [CxxToolchainInfo]),
    },
    artifact_promise_mappings = {
        "shared_library_interface": lambda p: p[_InterfaceInfo].artifact,
    },
)

def shared_library_interface(ctx: AnalysisContext, shared_lib: Artifact, anonymous: bool = False) -> Artifact:
    output = shared_library_interface_path(shared_lib.short_path)

    if anonymous:
        shared_lib_interface_artifact = ctx.actions.anon_target(
            _anon_shared_library_interface,
            dict(
                _cxx_toolchain = ctx.attrs._cxx_toolchain,
                output = output,
                shared_lib = shared_lib,
                identifier = shared_lib.short_path,
            ),
        ).artifact("shared_library_interface")
        shared_lib_interface_artifact = ctx.actions.assert_has_content_based_path(shared_lib_interface_artifact)
        return ctx.actions.assert_short_path(shared_lib_interface_artifact, short_path = output)
    else:
        return _shared_library_interface(
            ctx = ctx,
            output = output,
            shared_lib = shared_lib,
            identifier = shared_lib.short_path,
        )
