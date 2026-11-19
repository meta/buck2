# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference")
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",
    "LinkOrdering",
)

CxxLinkResultType = enum(
    "executable",
    "shared_library",
)

ExtraLinkerOutputCategory = enum(
    "produced-during-local-link",
    "produced-during-distributed-thin-lto-native-link",
    "produced-during-distributed-thin-lto-opt",
)

LinkOptions = record(
    # link args for binary link step
    binary_links = field(list[LinkArgs]),
    # common link args
    links = list[LinkArgs],
    link_execution_preference = LinkExecutionPreference,
    # Execution preference for the `dwp` action that packages the split debug
    # info of the linked output, if the toolchain produces one.
    #
    # `None` (the default) means "whatever the link itself uses", which is the
    # historical behavior: dwp ran alongside the link to avoid round-tripping
    # dwo files. Set this explicitly when the link has to be local for reasons
    # that do not apply to dwp (e.g. build-info stamping), so that dwp is not
    # dragged onto the local host with it.
    dwp_execution_preference = field([LinkExecutionPreference, None], None),
    link_weight = int,
    link_ordering = [LinkOrdering, None],
    enable_distributed_thinlto = bool,
    # A category suffix that will be added to the category of the link action that is generated.
    category_suffix = [str, None],
    # An identifier that will uniquely name this link action in the context of a category. Useful for
    # differentiating multiple link actions in the same rule.
    identifier = [str, None],
    strip = bool,
    # A function/lambda which will generate the strip args using the ctx.
    strip_args_factory = [typing.Callable, None],
    import_library = Artifact | None,
    # `None` expresses no preference, leaving the choice to buck2
    allow_cache_upload = [bool, None],
    cxx_toolchain = [CxxToolchainInfo, None],
    # If set, capture the local link action's stderr in this output while
    # continuing to stream it to the action's stderr.
    stderr_output = field(OutputArtifact | None, None),
    # Force callers to use link_options() or merge_link_options() to create.
    __private_use_link_options_function_to_construct = None,
    error_handler = [typing.Callable, None],
    # Factory methods used to provide extra linker outputs and flags, to
    # both local and distributed links.
    extra_linker_outputs_factory = field(typing.Callable | None, None),
    extra_linker_outputs_flags_factory = field(typing.Callable | None, None),
    extra_distributed_thin_lto_opt_outputs_merger = field(typing.Callable | None, None),
    produce_shared_library_interface = field(bool, False),
    incremental_link = field(bool, False),
    has_hip_device_debug = field(bool, False),
)

def link_options(
    links: list[LinkArgs],
    link_execution_preference: LinkExecutionPreference,
    dwp_execution_preference: [LinkExecutionPreference, None] = None,
    link_weight: int = 1,
    binary_links: list[LinkArgs] = [],
    link_ordering: [LinkOrdering, None] = None,
    enable_distributed_thinlto: bool = False,
    category_suffix: [str, None] = None,
    identifier: [str, None] = None,
    strip: bool = False,
    strip_args_factory = None,
    import_library: Artifact | None = None,
    allow_cache_upload: [bool, None] = False,
    cxx_toolchain: [CxxToolchainInfo, None] = None,
    stderr_output: OutputArtifact | None = None,
    error_handler: [typing.Callable, None] = None,
    extra_linker_outputs_factory: typing.Callable | None = None,
    extra_linker_outputs_flags_factory: typing.Callable | None = None,
    extra_distributed_thin_lto_opt_outputs_merger: typing.Callable | None = None,
    produce_shared_library_interface: bool = False,
    incremental_link: bool = False,
    has_hip_device_debug: bool = False,
) -> LinkOptions:
    """
    A type-checked constructor for LinkOptions because by default record
    constructors aren't typed.
    """
    return LinkOptions(
        binary_links = binary_links,
        links = links,
        link_execution_preference = link_execution_preference,
        dwp_execution_preference = dwp_execution_preference,
        link_weight = link_weight,
        link_ordering = link_ordering,
        enable_distributed_thinlto = enable_distributed_thinlto,
        category_suffix = category_suffix,
        identifier = identifier,
        strip = strip,
        strip_args_factory = strip_args_factory,
        import_library = import_library,
        allow_cache_upload = allow_cache_upload,
        cxx_toolchain = cxx_toolchain,
        stderr_output = stderr_output,
        __private_use_link_options_function_to_construct = None,
        error_handler = error_handler,
        extra_linker_outputs_factory = extra_linker_outputs_factory,
        extra_linker_outputs_flags_factory = extra_linker_outputs_flags_factory,
        extra_distributed_thin_lto_opt_outputs_merger = extra_distributed_thin_lto_opt_outputs_merger,
        produce_shared_library_interface = produce_shared_library_interface,
        incremental_link = incremental_link,
        has_hip_device_debug = has_hip_device_debug,
    )

# A marker instance to differentiate explicitly-passed None and a field that
# isn't provided in merge_link_options.
_NotProvided = record()
_NOT_PROVIDED = _NotProvided()

def merge_link_options(
    base: LinkOptions,
    binary_links: [list[LinkArgs], _NotProvided] = _NOT_PROVIDED,
    links: [list[LinkArgs], _NotProvided] = _NOT_PROVIDED,
    link_execution_preference: [LinkExecutionPreference, _NotProvided] = _NOT_PROVIDED,
    dwp_execution_preference: [LinkExecutionPreference, None, _NotProvided] = _NOT_PROVIDED,
    link_weight: [int, _NotProvided] = _NOT_PROVIDED,
    link_ordering: [LinkOrdering, None, _NotProvided] = _NOT_PROVIDED,
    enable_distributed_thinlto: [bool, _NotProvided] = _NOT_PROVIDED,
    category_suffix: [str, None, _NotProvided] = _NOT_PROVIDED,
    identifier: [str, None, _NotProvided] = _NOT_PROVIDED,
    strip: [bool, _NotProvided] = _NOT_PROVIDED,
    strip_args_factory = _NOT_PROVIDED,
    import_library: [Artifact, None, _NotProvided] = _NOT_PROVIDED,
    allow_cache_upload: [bool, None, _NotProvided] = _NOT_PROVIDED,
    cxx_toolchain: [CxxToolchainInfo, _NotProvided] = _NOT_PROVIDED,
    stderr_output: [OutputArtifact, None, _NotProvided] = _NOT_PROVIDED,
    incremental_link: [bool, _NotProvided] = _NOT_PROVIDED,
) -> LinkOptions:
    """
    Also something we would ideally auto-generate as LinkOptions.merge in
    Starlark.
    """

    return LinkOptions(
        binary_links = base.binary_links if binary_links == _NOT_PROVIDED else binary_links,
        links = base.links if links == _NOT_PROVIDED else links,
        link_execution_preference = base.link_execution_preference if link_execution_preference == _NOT_PROVIDED else link_execution_preference,
        dwp_execution_preference = base.dwp_execution_preference if dwp_execution_preference == _NOT_PROVIDED else dwp_execution_preference,
        link_weight = base.link_weight if link_weight == _NOT_PROVIDED else link_weight,
        link_ordering = base.link_ordering if link_ordering == _NOT_PROVIDED else link_ordering,
        enable_distributed_thinlto = base.enable_distributed_thinlto if enable_distributed_thinlto == _NOT_PROVIDED else enable_distributed_thinlto,
        category_suffix = base.category_suffix if category_suffix == _NOT_PROVIDED else category_suffix,
        identifier = base.identifier if identifier == _NOT_PROVIDED else identifier,
        strip = base.strip if strip == _NOT_PROVIDED else strip,
        strip_args_factory = base.strip_args_factory if strip_args_factory == _NOT_PROVIDED else strip_args_factory,
        import_library = base.import_library if import_library == _NOT_PROVIDED else import_library,
        allow_cache_upload = base.allow_cache_upload if allow_cache_upload == _NOT_PROVIDED else allow_cache_upload,
        cxx_toolchain = base.cxx_toolchain if cxx_toolchain == _NOT_PROVIDED else cxx_toolchain,
        stderr_output = base.stderr_output if stderr_output == _NOT_PROVIDED else stderr_output,
        __private_use_link_options_function_to_construct = None,
        error_handler = base.error_handler,
        extra_linker_outputs_factory = base.extra_linker_outputs_factory,
        extra_linker_outputs_flags_factory = base.extra_linker_outputs_flags_factory,
        extra_distributed_thin_lto_opt_outputs_merger = base.extra_distributed_thin_lto_opt_outputs_merger,
        produce_shared_library_interface = base.produce_shared_library_interface,
        incremental_link = base.incremental_link if incremental_link == _NOT_PROVIDED else incremental_link,
        has_hip_device_debug = base.has_hip_device_debug,
    )

def get_dwp_execution_preference(opts: LinkOptions) -> LinkExecutionPreference:
    """
    The execution preference the `dwp` action for this link should run with.

    Defaults to the link's own preference, so callers that do not care keep
    dwp next to the link.
    """
    if opts.dwp_execution_preference != None:
        return opts.dwp_execution_preference
    return opts.link_execution_preference
