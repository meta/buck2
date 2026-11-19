# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# This file contains the specification for all the providers the Erlang
# integration uses.

StartTypeValues = ["permanent", "transient", "temporary", "load", "none"]

StartType = enum(*StartTypeValues)

CodePathEntry = record(
    dir = field(Artifact),
    # application directories contribute `<dir>/ebin`, test output directories contribute `<dir>`
    ebin = field(bool),
)

def _code_path_projection(entry: CodePathEntry) -> cmd_args:
    if entry.ebin:
        return cmd_args(entry.dir, format = "{}/ebin")
    return cmd_args(entry.dir)

CodePathTSet = transitive_set(args_projections = {"code_path": _code_path_projection})

def code_path_args(code_path_tset: CodePathTSet) -> TransitiveSetArgsProjection:
    """The directories of a `CodePathTSet`, as they are passed to `erl -pa`."""
    return code_path_tset.project_as_args("code_path")

# Information about an Erlang application and its dependencies.

ErlangAppCommonFields = [
    # application name
    "name",
    # mapping from ("application", "basename") -> to header artifact
    "includes",
    # path to include directory
    "include_dir",
    # deps files short_path -> artifact
    "header_deps_file",
]

# target type to break circular dependencies
ErlangAppIncludeInfo = provider(
    fields = ErlangAppCommonFields
    + [
        # original inclues, used for validating _include_only app against the original
        "_original_includes",
    ],
)

ErlangAppInfo = provider(
    fields = ErlangAppCommonFields
    + [
        # version
        "version",
        # mapping from module name to beam artifact
        "beams",
        # for tests we need to preserve the private includes
        "private_includes",
        "private_include_dir",
        # mapping from name to dependency for all Erlang dependencies
        "dependencies",
        # CodePathTSet rooted at this application, None for virtual applications
        "code_path_tset",
        # Transitive Set for calculating the start order
        "start_dependencies",
        # additional targets that the application depends on, the
        # default output will end up in priv/
        "resources",
        # applications that are in path but not build by buck2 are virtual
        # the use-case for virtual apps are OTP applications that are shipeped
        # with the Erlang distribution
        "virtual",
        # app_folder
        "app_folder",
    ],
)

# mapping
#   from include base name (e.g. "header.hrl")
#   to artifact
PathArtifactMapping = dict[str, Artifact]

# mapping
#   from application to includes in that application
#   the artifacts are source .hrl files
IncludesMapping = dict[str, PathArtifactMapping]

# mapping
#   from module name to beam file location
ModuleArtifactMapping = dict[str, Artifact]

# mapping
#   from application to beam files it defines
EbinMapping = dict[str, ModuleArtifactMapping]

# mapping
#   from application to merged deps files
DepsMapping = dict[str, Artifact]

ErlangDependencyInfo = provider(
    fields = {
        "beams": provider_field(EbinMapping, default = {}),
        # code path of the transitive closure of the dependencies
        "code_path_tset": provider_field(CodePathTSet),
        "dependencies": provider_field(dict[str, Dependency], default = {}),
        "header_deps_files": provider_field(DepsMapping, default = {}),
        "include_dirs": provider_field(PathArtifactMapping, default = {}),
        "includes": provider_field(IncludesMapping, default = {}),
        "private_include_dirs": provider_field(PathArtifactMapping, default = {}),
        "private_includes": provider_field(IncludesMapping, default = {}),
    },
)

ErlangReleaseInfo = provider(
    fields = {
        "name": provider_field(typing.Any, default = None),
    },
)

# Marker provider to enforce what erlang tests can depend on
ErlangAppOrTestInfo = provider(fields = {})

Tool = cmd_args

ErlangOTPBinariesInfo = provider(
    fields = {
        "erl": provider_field(Tool),
        "erlc": provider_field(Tool),
        "escript": provider_field(Tool),
    },
)

Tools = record(
    erl = field(Tool),
    erlc = field(Tool),
    escript = field(Tool),
)

ErtsToolchainInfo = provider(
    fields = {
        # the directory each OTP application ships as, by application name
        "applications": provider_field(dict[str, Artifact]),
        # directory holding `erts-<version>`
        "erts": provider_field(Artifact),
        "headers": provider_field(Artifact),
        "otp_no_dot_erlang_boot": provider_field(Artifact),
        "otp_start_boot": provider_field(Artifact),
        # json, `{"erts_version": ..., "applications": {<application>: <version>}}`
        "versions": provider_field(Artifact),
    },
)

ErlangErrorHandlers = record(
    erlc = field(typing.Callable[[ActionErrorCtx], list[ActionSubError]]),
)

# toolchain provider
ErlangToolchainInfo = provider(
    # @unsorted-dict-items
    fields = {
        "name": provider_field(str),
        # command line erlc options used when compiling
        "erl_opts": provider_field(list[str]),
        # emulator flags used when calling erl
        "emu_flags": provider_field(list[str]),
        # struct containing the binaries erlc, escript, and erl
        # this is further split into local and RE
        "otp_binaries": provider_field(Tools),
        # utility scripts
        # building .app file
        "app_src_script": provider_field(Tool),
        # building .appup file
        "appup_src_script": provider_field(Tool),
        # building escripts
        "escript_builder": provider_field(Tool),
        # analyzing .(h|e)rl dependencies
        "dependency_analyzer": provider_field(Tool),
        "dependency_finalizer": provider_field(Tool),
        "dependency_merger": provider_field(Tool),
        "escript_trampoline": provider_field(Artifact),
        # name to parse_transform artifacts mapping for core parse_transforms (that are always used) and
        # user defines ones
        "core_parse_transforms": provider_field(dict[str, cmd_args]),
        "parse_transforms": provider_field(dict[str, cmd_args]),
        # filter spec for parse transforms
        "parse_transforms_filters": provider_field(dict[str, list[str]]),
        # release boot script builder
        "boot_script_builder": provider_field(Tool),
        # build release_variables
        "release_variables_builder": provider_field(Tool),
        # beams we need for various reasons
        "utility_modules": provider_field(Artifact),
        # env to be set for toolchain invocations
        "env": provider_field(dict[str, str]),
        # Erlang Runtime System (ERTS) toolchain metadata
        "erts_toolchain_info": provider_field(ErtsToolchainInfo | None, default = None),
        # error handler
        "error_handler": provider_field(ErlangErrorHandlers),
    },
)

# multi-version toolchain
ErlangMultiVersionToolchainInfo = provider(
    # @unsorted-dict-items
    fields = {
        "toolchain": provider_field(ErlangToolchainInfo),
    },
)

# parse_transform
ErlangParseTransformInfo = provider(
    # @unsorted-dict-items
    fields = {
        # module implementing the parse_transform
        "source": provider_field(Artifact),
        # potential extra files placed in a resource folder
        "extra_files": provider_field(list[Artifact]),
    },
)

ErlangTestInfo = provider(
    # @unsorted-dict-items
    fields = {
        # mapping from name to dependency for all Erlang dependencies
        "dependencies": provider_field(dict[str, Dependency]),
        # CodePathTSet rooted at this test
        "code_path_tset": provider_field(CodePathTSet),
        # The name of the suite
        "name": provider_field(str),
        # anchor to the output_dir
        "output_dir": provider_field(Artifact),
    },
)
