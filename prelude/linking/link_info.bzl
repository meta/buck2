# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",
    "make_artifact_tset",
)
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "LinkerInfo",
    "LinkerType",
    "PicBehavior",
)
load(
    "@prelude//cxx:linker.bzl",
    "get_link_whole_args",
    "get_no_as_needed_shared_libs_flags",
    "get_objects_as_library_args",
)
load(
    "@prelude//cxx:transformation_spec.bzl",
    "TransformationKind",
    "TransformationSpecContext",  # @unused Used as a type
)
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//utils:arglike.bzl", "ArgLike")

ExtraLinkerOutputs = record(
    # The unbound extra outputs produced by a link action
    # stored by key for lookup in the flag factory.
    artifacts = field(dict[str, Artifact], {}),
    # The output providers for the extra linker output.
    providers = field(dict[str, list[DefaultInfo]], {}),
)

ArchiveContentsType = enum(
    "normal",  # The static archive itself contains a symbol table and compressed object files
    "thin",  # The static archive only has a symbol table and refers to object files elsewhere on disk
    "virtual",  # Object files are passed directly to the linker between --start-lib --end-lib flags to indicate archive semantics, and a normal static archive is used by actions that request the archive itself
)

# Represents an archive (.a file)
Archive = record(
    archive_contents_type = field(ArchiveContentsType, default = ArchiveContentsType("normal")),
    artifact = field(Artifact),
    # Object files used to create the archive.
    external_objects = field(list[Artifact], []),
    # Inputs required when a consumer links external_objects instead of artifact.
    hidden = field(list[Artifact], []),
    # `hidden`, plus a thin archive's `external_objects`, as one `cmd_args` for consuming links; `hidden` stays a list because anon links serialise it artifact by artifact.
    hidden_args = field(cmd_args | None, None),
)

# What a link of the archive has to track as inputs although they are not on
# its command line: a thin archive is linked through its object files, a
# virtual one through `hidden`. None when there is nothing to track. A list
# costs one entry per artifact, a nested `cmd_args` one entry in total, so a
# caller that already holds the objects as a `cmd_args` passes that.
def _archive_hidden_args(
    archive_contents_type: ArchiveContentsType, external_objects: list[Artifact], hidden: list[Artifact], object_args: cmd_args | None
) -> cmd_args | None:
    if archive_contents_type == ArchiveContentsType("thin"):
        inputs = object_args if object_args != None else external_objects
    elif archive_contents_type == ArchiveContentsType("virtual"):
        inputs = hidden
    else:
        inputs = []
    return cmd_args(hidden = inputs) if inputs else None

def new_archive(
    artifact: Artifact, archive_contents_type: str, external_objects: list[Artifact] = [], hidden: list[Artifact] = [], object_args: cmd_args | None = None
) -> Archive:
    contents_type = ArchiveContentsType(archive_contents_type)
    return Archive(
        artifact = artifact,
        archive_contents_type = contents_type,
        external_objects = external_objects,
        hidden = hidden,
        hidden_args = _archive_hidden_args(contents_type, external_objects, hidden, object_args),
    )

# The different strategies that are used to determine sharedlib/executable
# link constituents.
# These default link strategies will each traverse dependenciess and collect
# all transitive dependencies that will link as archives until it reaches ones that will
# link as shared libs. For each dependency, this link strategy and that dependency's
# preferred_linkage will determine which LibOutputStyle will be used.
LinkStrategy = enum(
    # Prefers that dependencies be included in the link line as archives of native objects
    "static",
    # Prefers that dependencies be included in the link line as archives of PIC native objects
    "static_pic",
    # Prefers that dependencies be included in the link line as shared libraries
    "shared",
)

# A legacy type that was previously used to encode both LibOutputStyle and LinkStrategy. Uses should
# be updated to use the appropriate new enum.
LinkStyle = enum(
    "static",
    "static_pic",
    "shared",
)

# We still read link_style (and some related) attrs to LinkStyle but then usually convert them
# immediately to LinkStrategy.
# TODO(cjhopman): We should migrate the attr name to `link_strategy`
def to_link_strategy(link_style: LinkStyle) -> LinkStrategy:
    return LinkStrategy(link_style.value)

# The different types of outputs of native library. Which specific output style to use for a library
# will depend on the link_strategy that is being computed and the library's preferred_linkage.
LibOutputStyle = enum(
    "archive",
    "pic_archive",
    "shared_lib",
)

def default_output_style_for_link_strategy(link_strategy: LinkStrategy) -> LibOutputStyle:
    if link_strategy == LinkStrategy("static"):
        return LibOutputStyle("archive")
    if link_strategy == LinkStrategy("static_pic"):
        return LibOutputStyle("pic_archive")
    return LibOutputStyle("shared_lib")

# An archive.
ArchiveLinkable = record(
    # Artifact in the .a format from ar
    archive = field(Archive),
    # If a bitcode bundle was created for this artifact it will be present here
    bitcode_bundle = field(Artifact | None, None),
    linker_type = field(LinkerType),
    link_whole = field(bool, False),
    # Indicates if this archive may contain LTO bit code.  Can be set to `False`
    # to e.g. tell dist LTO handling that a potentially expensive archive doesn't
    # need to be processed.
    supports_lto = field(bool, True),
)

# A shared lib.
SharedLibLinkable = record(
    lib = field(Artifact),
    link_without_soname = field(bool, False),
)

# A list of objects.
ObjectsLinkable = record(
    objects = field([list[Artifact], None], None),
    # Any of the objects that are in bitcode format
    bitcode_bundle = field(Artifact | None, None),
    linker_type = field(LinkerType),
    link_whole = field(bool, False),
)

# Framework + library information for Apple/Cxx targets.
FrameworksLinkable = record(
    # An untrimmed list of framework paths, used to construct `-framework` args.
    frameworks = field(typing.Iterable, []),
    # A untrimmed list of library names, used to construct `-l` args.
    libraries = field(typing.Iterable, []),
    #
    ## WHY ITERABLES? ##
    #
    # For leaf nodes we just insert the attrs.frameworks + attrs.libraries
    # interned strings/lists.
    # They get dedupped into sets as they propagate.
)

_EMPTY_FRAMEWORKS_LINKABLE = FrameworksLinkable()

SwiftmoduleLinkable = record(
    swiftmodules = field(ArtifactTSet, ArtifactTSet()),
)

_EMPTY_SWIFTMODULE_LINKABLE = SwiftmoduleLinkable()

_EMPTY_FRAMEWORKS_LINKABLE_BY_STRATEGY = {link_strategy: _EMPTY_FRAMEWORKS_LINKABLE for link_strategy in LinkStrategy}
_EMPTY_SWIFTMODULE_LINKABLE_BY_STRATEGY = {link_strategy: _EMPTY_SWIFTMODULE_LINKABLE for link_strategy in LinkStrategy}

LinkableTypes = [
    ArchiveLinkable,
    SharedLibLinkable,
    ObjectsLinkable,
    FrameworksLinkable,
    SwiftmoduleLinkable,
]

LinkerFlags = record(
    flags = field(list[typing.Any], []),
    post_flags = field(list[typing.Any], []),
    exported_flags = field(list[typing.Any], []),
    exported_post_flags = field(list[typing.Any], []),
)

DepMetadata = record(
    # The version of a particular library linked into a binary. This is reported by
    # attributes on the library target itself.
    version = field(str),
)

# Contains the information required to add an item (often corresponding to a single library) to a link command line.
LinkInfo = record(
    # An informative name for this LinkInfo. This may be used in user messages
    # or when constructing intermediate output paths and does not need to be unique.
    name = field([str, None], None),
    dist_thin_lto_codegen_flags = field(list[typing.Any], []),
    # Opaque cmd_arg-likes to be added pre/post this item on a linker command line.
    pre_flags = field(list[typing.Any], []),
    post_flags = field(list[typing.Any], []),
    # Primary input to the linker, one of the Linkable types above.
    linkables = field(list[LinkableTypes], []),
    # Debug info which is referenced -- but not included -- by linkables in the
    # link info.  For example, this may include `.dwo` files, or the original
    # `.o` files if they contain debug info that doesn't follow the link.
    external_debug_info = field(ArtifactTSet, ArtifactTSet()),
    # Metadata attached to this LinkInfo. This metadata is propagated up the graph to
    # root nodes (like binaries).
    metadata = field(list[DepMetadata], []),
    has_hip_device_debug = field(bool, False),
    hip_arch_debug_files = field(dict[str, list[Artifact]], {}),
)

# The ordering to use when traversing linker libs transitive sets.
LinkOrdering = enum(
    # Preorder traversal, the default behavior which traverses depth-first returning the current
    # node, and then its children left-to-right.
    "preorder",
    # Topological sort, such that nodes are listed after all nodes that have them as descendants.
    "topological",
)

CxxSanitizerRuntimeInfo = provider(
    fields = {
        "runtime_files": provider_field(list[Artifact]),
    }
)

def set_link_info_link_whole(info: LinkInfo) -> LinkInfo:
    linkables = [set_linkable_link_whole(linkable) for linkable in info.linkables]
    return LinkInfo(
        name = info.name,
        pre_flags = info.pre_flags,
        post_flags = info.post_flags,
        linkables = linkables,
        external_debug_info = info.external_debug_info,
        metadata = info.metadata,
        has_hip_device_debug = info.has_hip_device_debug,
        hip_arch_debug_files = info.hip_arch_debug_files,
    )

def set_linkable_link_whole(
    linkable: [ArchiveLinkable, ObjectsLinkable, SharedLibLinkable, FrameworksLinkable],
) -> [ArchiveLinkable, ObjectsLinkable, SharedLibLinkable, FrameworksLinkable]:
    if isinstance(linkable, ArchiveLinkable):
        return ArchiveLinkable(
            archive = linkable.archive,
            linker_type = linkable.linker_type,
            link_whole = True,
            supports_lto = linkable.supports_lto,
        )
    elif isinstance(linkable, ObjectsLinkable):
        return ObjectsLinkable(
            objects = linkable.objects,
            linker_type = linkable.linker_type,
            link_whole = True,
        )
    return linkable

# Helper to wrap a LinkInfo with additional pre/post-flags.
def wrap_link_info(inner: LinkInfo, pre_flags: list[typing.Any] = [], post_flags: list[typing.Any] = []) -> LinkInfo:
    pre_flags = pre_flags + inner.pre_flags
    post_flags = inner.post_flags + post_flags
    return LinkInfo(
        name = inner.name,
        pre_flags = pre_flags,
        post_flags = post_flags,
        linkables = inner.linkables,
        external_debug_info = inner.external_debug_info,
        metadata = inner.metadata,
        has_hip_device_debug = inner.has_hip_device_debug,
        hip_arch_debug_files = inner.hip_arch_debug_files,
    )

def _is_linkable_comprised_of_object_files_or_a_lazy_archive(linkable: LinkableTypes) -> bool:
    return isinstance(linkable, ObjectsLinkable) or (isinstance(linkable, ArchiveLinkable) and not linkable.link_whole)

def serialize_linkable_for_thinlto(linkable: LinkableTypes) -> dict[str, typing.Any]:
    """Serialize the `linkable` so that it can be used in the ThinLTO metadata file."""
    object = {}
    if isinstance(linkable, ArchiveLinkable):
        object["type"] = "archive"
        object["archive"] = linkable.archive.artifact
    elif isinstance(linkable, SharedLibLinkable):
        object["type"] = "shared_lib"
        object["name"] = linkable.lib
    elif isinstance(linkable, ObjectsLinkable):
        # This path should NOT be reached.
        # ObjectLinkables are managed as bitcode, not as generic linkables.
        fail("ObjectsLinkable are sent as _BitcodeLinkData {}".format(str(linkable)))
    elif isinstance(linkable, FrameworksLinkable) or isinstance(linkable, SwiftmoduleLinkable):
        fail("Apple Frameworks linkables are not supported on GNU {}".format(str(linkable)))
    else:
        fail("Encountered unhandled linkable {}".format(str(linkable)))

    return object

# Adds appropriate args representing `linkable` to `args`
def append_linkable_args(args: cmd_args, linkable: LinkableTypes):
    if isinstance(linkable, ArchiveLinkable):
        if linkable.archive.archive_contents_type == ArchiveContentsType("virtual"):
            if not linkable.link_whole:
                args.add("-Wl,--start-lib")

            for object in linkable.archive.external_objects:
                args.add(object)

            if not linkable.link_whole:
                args.add("-Wl,--end-lib")
        elif linkable.link_whole:
            args.add(get_link_whole_args(linkable.linker_type, [linkable.archive.artifact]))
        else:
            args.add(linkable.archive.artifact)

        if linkable.archive.hidden_args != None:
            args.add(linkable.archive.hidden_args)
    elif isinstance(linkable, SharedLibLinkable):
        if linkable.link_without_soname:
            linkable_name = linkable.lib.basename.removeprefix("lib").split(".")[0]
            args.add(cmd_args(linkable.lib, format = "-L{}", parent = 1))
            args.add("-l" + linkable_name)
        else:
            args.add(linkable.lib)
    elif isinstance(linkable, ObjectsLinkable):
        # We need to export every symbol when link groups are used, but enabling
        # --whole-archive with --start-lib is undefined behavior in gnu linkers:
        # https://reviews.llvm.org/D120443. We need to export symbols from every
        # linkable in the link_info
        if not linkable.link_whole:
            args.add(get_objects_as_library_args(linkable.linker_type, linkable.objects))
        else:
            args.add(linkable.objects)
    elif isinstance(linkable, FrameworksLinkable) or isinstance(linkable, SwiftmoduleLinkable):
        # These flags are handled separately so they can be deduped.
        #
        # We've seen in apps with larger dependency graphs that failing
        # to dedupe these args results in linker.argsfile which are too big.
        pass
    else:
        fail("Encountered unhandled linkable {}".format(str(linkable)))

# The artifacts `append_linkable_args` puts on the link line for `value`, in
# order: an archive, or the objects of a virtual archive; a shared library, or
# the interface the toolchain links against instead; every object of an
# objects linkable. Frameworks and swiftmodules contribute none. Kept next to
# `append_linkable_args` so the two agree.
def link_info_outputs(value: LinkInfo) -> list[Artifact]:
    outputs = []
    for linkable in value.linkables:
        if isinstance(linkable, ArchiveLinkable):
            if linkable.archive.archive_contents_type == ArchiveContentsType("virtual"):
                outputs.extend(linkable.archive.external_objects)
            else:
                outputs.append(linkable.archive.artifact)
        elif isinstance(linkable, SharedLibLinkable):
            outputs.append(linkable.lib)
        elif isinstance(linkable, ObjectsLinkable):
            outputs.extend(linkable.objects or [])
        elif isinstance(linkable, FrameworksLinkable) or isinstance(linkable, SwiftmoduleLinkable):
            pass
        else:
            fail("Encountered unhandled linkable {}".format(str(linkable)))
    return outputs

LinkInfoArgumentFilter = enum(
    "all",
    "object_files_and_lazy_archives_only",
    "exclude_object_files_and_lazy_archives",
)

def link_info_to_args(value: LinkInfo, argument_type_filter: LinkInfoArgumentFilter = LinkInfoArgumentFilter("all"), args: cmd_args | None = None) -> cmd_args:
    """Append selected link arguments to args, or create a new command line, and return it."""
    result = args if args != None else cmd_args()

    do_pre_post_flags = argument_type_filter == LinkInfoArgumentFilter("all") or argument_type_filter == LinkInfoArgumentFilter(
        "exclude_object_files_and_lazy_archives"
    )
    if do_pre_post_flags:
        result.add(value.pre_flags)

    for linkable in value.linkables:
        if argument_type_filter == LinkInfoArgumentFilter("all"):
            append_linkable_args(result, linkable)

        elif argument_type_filter == LinkInfoArgumentFilter("object_files_and_lazy_archives_only") and _is_linkable_comprised_of_object_files_or_a_lazy_archive(
            linkable
        ):
            append_linkable_args(result, linkable)

        elif argument_type_filter == LinkInfoArgumentFilter(
            "exclude_object_files_and_lazy_archives"
        ) and not _is_linkable_comprised_of_object_files_or_a_lazy_archive(linkable):
            append_linkable_args(result, linkable)

    if do_pre_post_flags:
        result.add(value.post_flags)

    return result

def _link_info_to_tbd_creation_args(value: LinkInfo) -> cmd_args:
    result = cmd_args()
    for linkable in value.linkables:
        if isinstance(linkable, ArchiveLinkable):
            if linkable.archive.external_objects:
                for object in linkable.archive.external_objects:
                    result.add(object)
            else:
                result.add(linkable.archive.artifact)
            result.add(cmd_args(hidden = linkable.archive.hidden))
        if isinstance(linkable, ObjectsLinkable):
            for object in linkable.objects:
                result.add(object)

    return result

LinkableFlavor = enum(
    # Provides compile outputs
    "default",
    # Provides compile outputs
    # using optimization flags from toolchain
    "optimized",
    # Provides compile outputs
    # using debug flags from toolchain
    "debug",
    # Provides compile outputs
    # stripped of debug symbols
    "stripped",
)

# Encapsulate all `LinkInfo`s provided by a given rule's link style.
#
# We provide both the "default" and (optionally) a pre-"stripped" LinkInfo. For a consumer that doesn't care
# about debug info (for example, who is going to produce stripped output anyway), it can be significantly
# cheaper to consume the pre-stripped LinkInfo.
LinkInfos = record(
    label = field(Label | None, None),
    # Link info to use by default.
    default = field(LinkInfo),
    # Link info for objects compiler with extra optimizations (EXPERIMENTAL)
    optimized = field([LinkInfo, None], None),
    # Link info for objects compiler with debug optimizations (-g2 + -O0)
    debuggable = field([LinkInfo, None], None),
    # Link info stripped of debug symbols.
    stripped = field([LinkInfo, None], None),
)

def _link_info_default_args(infos: LinkInfos) -> cmd_args:
    info = infos.default
    return link_info_to_args(info, argument_type_filter = LinkInfoArgumentFilter("all"))

def _link_info_stripped_link_args(infos: LinkInfos) -> cmd_args:
    info = infos.stripped or infos.default
    return link_info_to_args(info, argument_type_filter = LinkInfoArgumentFilter("all"))

def _link_info_object_files_and_lazy_archives_only_args(infos: LinkInfos) -> cmd_args:
    return link_info_to_args(infos.default, argument_type_filter = LinkInfoArgumentFilter("object_files_and_lazy_archives_only"))

def _link_info_excluding_object_files_and_lazy_archives_args(infos: LinkInfos) -> cmd_args:
    return link_info_to_args(infos.default, argument_type_filter = LinkInfoArgumentFilter("exclude_object_files_and_lazy_archives"))

def _link_info_tbd_creation_args(infos: LinkInfos) -> cmd_args:
    return _link_info_to_tbd_creation_args(infos.default)

def link_info_to_metadata_args(info: LinkInfo, args: cmd_args | None = None) -> ArgLike:
    if args == None:
        args = cmd_args()
    for meta in info.metadata:
        args.add("version:" + meta.version)
    return args

def _link_info_metadata_args(infos: LinkInfos):
    info = infos.stripped or infos.default
    return link_info_to_metadata_args(info)

# TransitiveSet of LinkInfos.
LinkInfosTSet = transitive_set(
    args_projections = {
        "default": _link_info_default_args,
        "exclude_object_files_and_lazy_archives": _link_info_excluding_object_files_and_lazy_archives_args,
        "metadata": _link_info_metadata_args,
        "object_files_and_lazy_archives_only": _link_info_object_files_and_lazy_archives_only_args,
        "stripped": _link_info_stripped_link_args,
        "tbd_creation_args": _link_info_tbd_creation_args,
    },
)

LinkArgsTSet = record(
    infos = field(LinkInfosTSet),
    external_debug_info = field(ArtifactTSet, ArtifactTSet()),
    prefer_stripped = field(bool, False),
)

# An enum. Only one field should be set. The variants here represent different
# ways in which we might obtain linker commands: through a t-set of propagated
# dependencies (used for deps propagated unconditionally up a tree), through a
# series of LinkInfo (used for link groups, Omnibus linking), or simply through
# raw arguments we want to include (used for e.g. per-target link flags).
LinkArgs = record(
    # A LinkInfosTSet + a flag indicating if stripped is preferred.
    tset = field([LinkArgsTSet, None], None),
    # A list of LinkInfos
    infos = field([list[LinkInfo], None], None),
    # A bunch of flags.
    flags = field([ArgLike, None], None),
)

# The output of a native link (e.g. a shared library or an executable).
LinkedObject = record(
    output = field([Artifact, Promise]),
    # The combined bitcode from this linked object and any static libraries
    bitcode_bundle = field(Artifact | None, None),
    # the generated linked output before running stripping(and bolt).
    unstripped_output = field(Artifact),
    # the generated linked output before running bolt, may be None if bolt is not used.
    prebolt_output = field(Artifact | None, None),
    # The LinkArgs used to produce this LinkedObject. This can be useful for debugging or
    # for downstream rules to reproduce the shared library with some modifications (for example
    # android relinker will link again with an added version script argument).
    link_args = field(list[LinkArgs] | None, None),
    # A linked object (binary/shared library) may have an associated dwp file with
    # its corresponding DWARF debug info.
    # May be None when Split DWARF is disabled or for some types of synthetic link objects.
    dwp = field(Artifact | None, None),
    # Per-gfx-arch sidecar `.debug` files keyed by arch label.
    # Not set by cxx_link_into(); populated by callers (cxx_library,
    # link_groups) from compile outputs after linking.
    hip_arch_debug_files = field(dict[str, list[Artifact]], {}),
    # Additional dirs or paths that contain debug info referenced by the linked
    # object (e.g. split dwarf files or PDB file).
    external_debug_info = field(ArtifactTSet, ArtifactTSet()),
    # This argsfile is generated in the `cxx_link` step and contains a list of arguments
    # passed to the linker. It is being exposed as a sub-target for debugging purposes.
    linker_argsfile = field(Artifact | None, None),
    # The linker command as generated by `cxx_link`. Exposed for debugging purposes only.
    # Not present for DistLTO scenarios.
    linker_command = field([cmd_args, None], None),
    # This sub-target is only available for distributed thinLTO builds.
    index_argsfile = field(Artifact | None, None),
    # Import library for linking with DLL on Windows.
    # If not on Windows it's always None.
    import_library = field(Artifact | None, None),
    # A linked object (binary/shared library) may have an associated PDB file with
    # its corresponding Windows debug info.
    # If not on Windows it's always None.
    pdb = field(Artifact | None, None),
    # Incremental linking state file (.ilk) for MSVC on Windows.
    ilk = field(Artifact | None, None),
    # Split-debug info generated by the link.
    split_debug_output = field(Artifact | None, None),
)

# A map of native linkable infos from transitive dependencies for each LinkStrategy.
# This contains the information about how to link in a target for each link strategy.
# This doesn't contain the information about things needed to package the linked result
# (i.e. this doesn't contain the information needed to know what shared libs needed at runtime
# for the final result).
MergedLinkInfo = provider(
    fields = {
        # Apple framework linker args must be deduped to avoid overflow in our argsfiles.
        #
        # To save on repeated computation of transitive LinkInfos, we store a dedupped
        # structure, based on the link-style.
        "frameworks": provider_field(dict[LinkStrategy, FrameworksLinkable | None]),
        "swiftmodules": provider_field(dict[LinkStrategy, SwiftmoduleLinkable | None]),
        "_external_debug_info": provider_field(dict[LinkStrategy, ArtifactTSet]),
        "_infos": provider_field(dict[LinkStrategy, LinkInfosTSet]),
    }
)

# A map of linkages to all possible output styles it supports.
_LIB_OUTPUT_STYLES_FOR_LINKAGE = {
    Linkage("any"): [LibOutputStyle("archive"), LibOutputStyle("pic_archive"), LibOutputStyle("shared_lib")],
    Linkage("static"): [LibOutputStyle("archive"), LibOutputStyle("pic_archive")],
    Linkage("shared"): [LibOutputStyle("pic_archive"), LibOutputStyle("shared_lib")],
}

# Helper to wrap a LinkInfos with additional pre/post-flags.
def wrap_link_infos(inner: LinkInfos, pre_flags: list[typing.Any] = [], post_flags: list[typing.Any] = []) -> LinkInfos:
    return LinkInfos(
        default = wrap_link_info(
            inner.default,
            pre_flags = pre_flags,
            post_flags = post_flags,
        ),
        stripped = None
        if inner.stripped == None
        else wrap_link_info(
            inner.stripped,
            pre_flags = pre_flags,
            post_flags = post_flags,
        ),
    )

def create_merged_link_info(
    # Target context for which to create the link info.
    ctx: AnalysisContext,
    pic_behavior: PicBehavior,
    # The outputs available for this rule, as a map from LibOutputStyle (as
    # used by dependents) to `LinkInfo`.
    link_infos: dict[LibOutputStyle, LinkInfos] = {},
    # How the rule requests to be linked.  This will be used to determine
    # which actual link style to propagate for each "requested" link style.
    preferred_linkage: Linkage = Linkage("any"),
    # Link info to propagate from non-exported deps for static link styles.
    deps: list[MergedLinkInfo] = [],
    # Link info to always propagate from exported deps.
    exported_deps: list[MergedLinkInfo] = [],
    frameworks_linkable: [FrameworksLinkable, None] = None,
    swiftmodule_linkable: [SwiftmoduleLinkable, None] = None,
) -> MergedLinkInfo:
    """
    Create a `MergedLinkInfo` provider.
    """

    infos = {}
    external_debug_info = {}
    frameworks = None
    swiftmodules = None

    # We don't know how this target will be linked, so we generate the possible
    # link info given the target's preferred linkage, to be consumed by the
    # ultimate linking target.
    for link_strategy in LinkStrategy:
        actual_output_style = get_lib_output_style(link_strategy, preferred_linkage, pic_behavior)

        children = []
        external_debug_info_children = []
        framework_linkables = []
        swiftmodule_linkables = []

        # When we're being linked statically, we also need to export all private
        # linkable input (e.g. so that any unresolved symbols we have are
        # resolved properly when we're linked).
        if actual_output_style != LibOutputStyle("shared_lib"):
            # We never want to propagate the linkables used to build a shared library.
            #
            # Doing so breaks the encapsulation of what is in linked in the library vs. the main executable.
            framework_linkables.append(frameworks_linkable)
            framework_linkables += [dep_info.frameworks[link_strategy] for dep_info in exported_deps]

            swiftmodule_linkables.append(swiftmodule_linkable)
            swiftmodule_linkables += [dep_info.swiftmodules[link_strategy] for dep_info in exported_deps]

            for dep_info in deps:
                # The inherited link infos no longer guarantees that a tset will be available for
                # all link strategies. Protect against missing infos
                value = dep_info._infos.get(link_strategy)
                if value:
                    children.append(value)
                value = dep_info._external_debug_info.get(link_strategy)
                if value:
                    external_debug_info_children.append(value)

                framework_linkables.append(dep_info.frameworks[link_strategy])
                swiftmodule_linkables.append(dep_info.swiftmodules[link_strategy])

        # We always export link info for exported deps.
        for dep_info in exported_deps:
            value = dep_info._infos.get(link_strategy)
            if value:
                children.append(value)
            value = dep_info._external_debug_info.get(link_strategy)
            if value:
                external_debug_info_children.append(value)

        merged_frameworks = _merge_framework_linkables(framework_linkables)
        if merged_frameworks != _EMPTY_FRAMEWORKS_LINKABLE:
            if frameworks == None:
                frameworks = dict(_EMPTY_FRAMEWORKS_LINKABLE_BY_STRATEGY)
            frameworks[link_strategy] = merged_frameworks

        merged_swiftmodules = merge_swiftmodule_linkables(ctx, swiftmodule_linkables)
        if merged_swiftmodules != _EMPTY_SWIFTMODULE_LINKABLE:
            if swiftmodules == None:
                swiftmodules = dict(_EMPTY_SWIFTMODULE_LINKABLE_BY_STRATEGY)
            swiftmodules[link_strategy] = merged_swiftmodules

        if actual_output_style in link_infos:
            link_info = link_infos[actual_output_style]

            # TODO(cjhopman): This seems like we won't propagate information about our children unless this target itself
            # has an output for this strategy. Why is that correct?
            infos[link_strategy] = ctx.actions.tset(
                LinkInfosTSet,
                value = link_info,
                children = children,
            )
            external_debug_info[link_strategy] = make_artifact_tset(
                actions = ctx.actions,
                label = ctx.label,
                children = ([link_info.default.external_debug_info] + external_debug_info_children),
            )

    return MergedLinkInfo(
        _infos = infos,
        _external_debug_info = external_debug_info,
        frameworks = frameworks if frameworks != None else _EMPTY_FRAMEWORKS_LINKABLE_BY_STRATEGY,
        swiftmodules = swiftmodules if swiftmodules != None else _EMPTY_SWIFTMODULE_LINKABLE_BY_STRATEGY,
    )

def create_merged_link_info_for_propagation(ctx: AnalysisContext, xs: list[MergedLinkInfo]) -> MergedLinkInfo:
    """
    Creates a MergedLinkInfo for a node that just propagates up its dependencies' MergedLinkInfo without contributing anything itself.

    A node that contributes something itself would use create_merged_link_info.
    """
    merged = {}
    merged_external_debug_info = {}
    frameworks = None
    swiftmodules = None
    for link_strategy in LinkStrategy:
        merged[link_strategy] = ctx.actions.tset(
            LinkInfosTSet,
            children = filter(None, [x._infos.get(link_strategy) for x in xs]),
        )
        merged_external_debug_info[link_strategy] = make_artifact_tset(
            actions = ctx.actions,
            label = ctx.label,
            children = filter(None, [x._external_debug_info.get(link_strategy) for x in xs]),
        )
        merged_frameworks = _merge_framework_linkables([x.frameworks[link_strategy] for x in xs])
        if merged_frameworks != _EMPTY_FRAMEWORKS_LINKABLE:
            if frameworks == None:
                frameworks = dict(_EMPTY_FRAMEWORKS_LINKABLE_BY_STRATEGY)
            frameworks[link_strategy] = merged_frameworks

        merged_swiftmodules = merge_swiftmodule_linkables(ctx, [x.swiftmodules[link_strategy] for x in xs])
        if merged_swiftmodules != _EMPTY_SWIFTMODULE_LINKABLE:
            if swiftmodules == None:
                swiftmodules = dict(_EMPTY_SWIFTMODULE_LINKABLE_BY_STRATEGY)
            swiftmodules[link_strategy] = merged_swiftmodules

    return MergedLinkInfo(
        _infos = merged,
        _external_debug_info = merged_external_debug_info,
        frameworks = frameworks if frameworks != None else _EMPTY_FRAMEWORKS_LINKABLE_BY_STRATEGY,
        swiftmodules = swiftmodules if swiftmodules != None else _EMPTY_SWIFTMODULE_LINKABLE_BY_STRATEGY,
    )

def get_link_info(infos: LinkInfos, prefer_stripped: bool = False, prefer_optimized: bool = False) -> LinkInfo:
    """
    Helper for getting a `LinkInfo` out of a `LinkInfos`.
    """

    # When requested, prefer using pre-stripped link info.
    if prefer_stripped and infos.stripped != None:
        return infos.stripped
    if prefer_optimized and infos.optimized:
        return infos.optimized

    return infos.default

def unpack_link_args_metadata(args: LinkArgs) -> ArgLike:
    if args.tset != None:
        return args.tset.infos.project_as_args("metadata")
    ret = cmd_args()
    if args.infos != None:
        for info in args.infos:
            link_info_to_metadata_args(info, ret)
    return ret

def dedupe_dep_metadata(metadatas: list[DepMetadata]) -> list[DepMetadata]:
    versions = set([m.version for m in metadatas])
    return [DepMetadata(version = v) for v in versions]

def truncate_dep_metadata(metadatas: list[DepMetadata]) -> list[DepMetadata]:
    """
    It's entirely possible we have way too much link metadata to put into buildinfo;
    let's truncate based on the first 512 bytes (counting strings).
    """
    max_size = 512
    size = 0
    for i, metadata in enumerate(metadatas):
        if size > max_size:
            return metadatas[:i]
        size += len(metadata.version)

    return metadatas

def link_args_metadata_with_flag(args: LinkArgs, link_metadata_flag: str | None = None) -> cmd_args:
    cmd = cmd_args()
    if link_metadata_flag:
        cmd.add(cmd_args(unpack_link_args_metadata(args), prepend = link_metadata_flag))
    return cmd

def unpack_link_args(args: LinkArgs, link_ordering: [LinkOrdering, None] = None, link_metadata_flag: str | None = None) -> ArgLike:
    cmd = link_args_metadata_with_flag(args, link_metadata_flag)
    if args.tset != None:
        ordering = link_ordering.value if link_ordering else "preorder"

        tset = args.tset.infos
        if args.tset.prefer_stripped:
            cmd.add(tset.project_as_args("stripped", ordering = ordering))
        else:
            cmd.add(tset.project_as_args("default", ordering = ordering))
        return cmd

    if args.infos != None:
        for info in args.infos:
            link_info_to_args(info, args = cmd)
        return cmd

    if args.flags != None:
        cmd.add(args.flags)
        return cmd

    fail("Unpacked invalid empty link args")

def unpack_link_args_excluding_object_files_and_lazy_archives(args: LinkArgs) -> [ArgLike, None]:
    if args.tset != None:
        if args.tset.prefer_stripped:
            fail("Preferring stripped link infos is not supported by this function.")

        return args.tset.infos.project_as_args("exclude_object_files_and_lazy_archives")

    if args.infos != None:
        result_args = cmd_args()
        for info in args.infos:
            result_args.add(link_info_to_args(info, argument_type_filter = LinkInfoArgumentFilter("exclude_object_files_and_lazy_archives")))
        return result_args

    if args.flags != None:
        return args.flags

    fail("Unpacked invalid empty link args")

def unpack_link_args_object_files_and_lazy_archives_only(args: LinkArgs) -> [ArgLike, None]:
    if args.tset != None:
        if args.tset.prefer_stripped:
            fail("Preferring stripped link infos is not supported by this function.")

        return args.tset.infos.project_as_args("object_files_and_lazy_archives_only")

    if args.infos != None:
        result_args = cmd_args()
        for info in args.infos:
            result_args.add(link_info_to_args(info, argument_type_filter = LinkInfoArgumentFilter("object_files_and_lazy_archives_only")))
        return result_args

    if args.flags != None:
        return None

    fail("Unpacked invalid empty link args")

def unpack_link_args_for_tbd_creation(args: LinkArgs) -> ArgLike:
    if args.tset != None:
        if args.tset.prefer_stripped:
            fail("Preferring stripped link infos is not supported by this function.")

        return args.tset.infos.project_as_args("tbd_creation_args")

    if args.infos != None:
        result_args = cmd_args()
        for info in args.infos:
            result_args.add(_link_info_to_tbd_creation_args(info))
        return result_args

    if args.flags != None:
        return None

    fail("Unpacked invalid empty link args")

def unpack_external_debug_info(actions: AnalysisActions, args: LinkArgs) -> ArtifactTSet:
    if args.tset != None:
        if args.tset.prefer_stripped:
            return ArtifactTSet()
        return args.tset.external_debug_info

    if args.infos != None:
        return make_artifact_tset(
            actions = actions,
            children = [info.external_debug_info for info in args.infos],
        )

    if args.flags != None:
        return ArtifactTSet()

    fail("Unpacked invalid empty link args")

def map_to_link_infos(links: list[LinkArgs]) -> list[LinkInfo]:
    res = []

    def append(v):
        if v.pre_flags or v.post_flags or v.dist_thin_lto_codegen_flags or v.linkables:
            res.append(v)

    for link in links:
        if link.tset != None:
            for info in link.tset.infos.traverse():
                if link.tset.prefer_stripped:
                    append(info.stripped or info.default)
                else:
                    append(info.default)
            continue
        if link.infos != None:
            for link in link.infos:
                append(link)
            continue
        if link.flags != None:
            append(LinkInfo(pre_flags = [link.flags]))
            continue
        fail("Unpacked invalid empty link args")
    return res

def get_link_args_for_strategy(
    actions: AnalysisActions,
    label: Label,
    linker_info: LinkerInfo,
    deps_merged_link_infos: list[MergedLinkInfo],
    link_strategy: LinkStrategy,
    prefer_stripped: bool,
    transformation_spec_context: TransformationSpecContext | None,
    additional_link_info: [LinkInfo, None] = None,
) -> LinkArgs:
    """
    Derive the `LinkArgs` for a strategy and strip preference from a list of dependency's MergedLinkInfo.
    """

    infos_kwargs = {}
    if additional_link_info:
        infos_kwargs = {"value": LinkInfos(default = additional_link_info, stripped = additional_link_info)}
    infos = actions.tset(LinkInfosTSet, children = filter(None, [x._infos.get(link_strategy) for x in deps_merged_link_infos]), **infos_kwargs)

    external_debug_info = make_artifact_tset(
        actions = actions,
        label = label,
        children = filter(
            None,
            [x._external_debug_info.get(link_strategy) for x in deps_merged_link_infos]
            + ([additional_link_info.external_debug_info] if additional_link_info else []),
        ),
    )

    if transformation_spec_context and not transformation_spec_context.provider.is_empty:
        link_ordering = linker_info.link_ordering or "preorder"
        flattened_results = []
        for link_infos in infos.traverse(ordering = link_ordering):
            link_info = get_link_info_for_transformation(
                transformation_spec_context,
                link_infos,
                link_infos.label,
                prefer_stripped,
            )
            flattened_results.append(link_info)
        return LinkArgs(
            infos = flattened_results,
        )

    return LinkArgs(
        tset = LinkArgsTSet(
            infos = infos,
            external_debug_info = external_debug_info,
            prefer_stripped = prefer_stripped,
        ),
    )

def _get_non_transformed_info(link_infos: LinkInfos, prefer_stripped: bool) -> LinkInfo:
    if prefer_stripped:
        return link_infos.stripped or link_infos.default
    return link_infos.default

def get_link_info_for_transformation(
    transformation_spec_context: TransformationSpecContext, link_infos: LinkInfos, label: Label | None, prefer_stripped: bool
) -> LinkInfo:
    info = None

    if label:
        transformation_kind = transformation_spec_context.provider.determine_transformation(label, transformation_spec_context.graph_info)
        if transformation_kind:
            if transformation_kind == TransformationKind("debug"):
                info = link_infos.debuggable
            elif transformation_kind == TransformationKind("optimized"):
                info = link_infos.optimized

    return info or _get_non_transformed_info(link_infos, prefer_stripped)

def get_lib_output_style(requested_link_strategy: LinkStrategy, preferred_linkage: Linkage, pic_behavior: PicBehavior) -> LibOutputStyle:
    """
    Return what lib output style to use for a library for a requested link style and preferred linkage.
    --------------------------------------------------------------
    | preferred_linkage |              link_strategy             |
    |                   |----------------------------------------|
    |                   | static   | static_pic   |   shared     |
    -------------------------------------------------------------|
    |      static       | *archive | *pic_archive | *pic_archive |
    |      shared       | shared   |   shared     |   shared     |
    |       any         | *archive | *pic_archive |   shared     |
    --------------------------------------------------------------

    Either of *static or *static_pic may be changed to the other based on the pic_behavior
    """
    no_pic_style = _get_lib_output_style_without_pic_behavior(requested_link_strategy, preferred_linkage)
    return process_output_style_for_pic_behavior(no_pic_style, pic_behavior)

def _get_lib_output_style_without_pic_behavior(requested_link_strategy: LinkStrategy, preferred_linkage: Linkage) -> LibOutputStyle:
    if preferred_linkage == Linkage("any"):
        return default_output_style_for_link_strategy(requested_link_strategy)
    elif preferred_linkage == Linkage("shared"):
        return LibOutputStyle("shared_lib")
    else:  # preferred_linkage = static
        if requested_link_strategy == LinkStrategy("static"):
            return LibOutputStyle("archive")
        else:
            return LibOutputStyle("pic_archive")

def process_link_strategy_for_pic_behavior(link_strategy: LinkStrategy, behavior: PicBehavior) -> LinkStrategy:
    """
    Converts static/static_pic link styles to the appropriate static form according to the pic behavior.
    """
    if link_strategy == LinkStrategy("shared"):
        return link_strategy
    elif behavior == PicBehavior("supported"):
        return link_strategy
    elif behavior == PicBehavior("not_supported"):
        return LinkStrategy("static")
    elif behavior == PicBehavior("always_enabled"):
        return LinkStrategy("static_pic")
    else:
        fail("Unknown pic_behavior: {}".format(behavior))

# TODO(cjhopman): I think we should be able to make it an error to request an output style that
# violates the PicBehavior if we consistently translate LinkStyle and other top-level requests (i.e.
# it seems like the need to translate output styles points to us missing a translation at some higher level).
def process_output_style_for_pic_behavior(output_style: LibOutputStyle, behavior: PicBehavior) -> LibOutputStyle:
    """
    Converts archive/archive_pic output styles to the appropriate output form according to the pic behavior.
    """
    if output_style == LibOutputStyle("shared_lib"):
        return output_style
    elif behavior == PicBehavior("supported"):
        return output_style
    elif behavior == PicBehavior("not_supported"):
        return LibOutputStyle("archive")
    elif behavior == PicBehavior("always_enabled"):
        return LibOutputStyle("pic_archive")
    else:
        fail("Unknown pic_behavior: {}".format(behavior))

def subtarget_for_output_style(output_style: LibOutputStyle) -> str:
    # TODO(cjhopman): This preserves historical strings for these (when we used LinkStyle for both link strategies and
    # output styles). It would be good to update that to match the LibOutputStyle.
    return legacy_output_style_to_link_style(output_style).value.replace("_", "-")

def get_output_styles_for_linkage(linkage: Linkage) -> list[LibOutputStyle]:
    """
    Return all possible `LibOutputStyle`s that apply for the given `Linkage`.
    """
    return _LIB_OUTPUT_STYLES_FOR_LINKAGE[linkage]

def legacy_output_style_to_link_style(output_style: LibOutputStyle) -> LinkStyle:
    """
    We previously used LinkStyle to represent both the type of a library output and for the different default link strategies.

    To support splitting those two concepts, some places are still using LinkStyle when they probably should be using LibOutputStyle.
    """
    if output_style == LibOutputStyle("shared_lib"):
        return LinkStyle("shared")
    elif output_style == LibOutputStyle("archive"):
        return LinkStyle("static")
    elif output_style == LibOutputStyle("pic_archive"):
        return LinkStyle("static_pic")
    fail("unrecognized output_style {}".format(output_style))

def has_framework_linkable(linkables: list[[FrameworksLinkable, None]]) -> bool:
    for linkable in linkables:
        if linkable:
            return True

    return False

def _merge_framework_linkables(linkables: list[[FrameworksLinkable, None]]) -> FrameworksLinkable:
    if not has_framework_linkable(linkables):
        return _EMPTY_FRAMEWORKS_LINKABLE

    unique_frameworks = set()
    unique_libraries = set()
    for linkable in linkables:
        if not linkable:
            continue

        unique_frameworks.update(linkable.frameworks)
        unique_libraries.update(linkable.libraries)

    # Non-None inputs can still be contentless (leaf nodes pass fresh empty
    # records rather than None); collapse to the shared empty.
    if not unique_frameworks and not unique_libraries:
        return _EMPTY_FRAMEWORKS_LINKABLE

    return FrameworksLinkable(
        frameworks = unique_frameworks,
        libraries = unique_libraries,
    )

def merge_swiftmodule_linkables(ctx: AnalysisContext, linkables: list[[SwiftmoduleLinkable, None]]) -> SwiftmoduleLinkable:
    # Contentless linkables (empty inner tset) contribute nothing; dropping
    # them here lets the all-empty and single-contributor cases share records
    # instead of re-wrapping per node.
    non_empty = [linkable for linkable in linkables if linkable != None and linkable.swiftmodules._tset != None]

    if not non_empty:
        return _EMPTY_SWIFTMODULE_LINKABLE

    if len(non_empty) == 1:
        return non_empty[0]

    return SwiftmoduleLinkable(
        swiftmodules = make_artifact_tset(
            actions = ctx.actions,
            label = ctx.label,
            children = [linkable.swiftmodules for linkable in non_empty],
        )
    )

def wrap_with_no_as_needed_shared_libs_flags(linker_type: LinkerType, link_info: LinkInfo) -> LinkInfo:
    """
    Wrap link info in args used to prevent linkers from dropping unused shared
    library dependencies from the e.g. DT_NEEDED tags of the link.
    """

    if linker_type == LinkerType("gnu"):
        return wrap_link_info(
            inner = link_info,
            pre_flags = (["-Wl,--push-state"] + get_no_as_needed_shared_libs_flags(linker_type)),
            post_flags = ["-Wl,--pop-state"],
        )

    if linker_type == LinkerType("darwin"):
        return link_info

    fail("Linker type {} not supported".format(linker_type))

# Represents information to debug linker commands. Used to carry information
# about link commands.
LinkCommandDebugOutput = record(
    # The filename of the linkable output.
    filename = str,
    command = ArgLike,
    argsfile = Artifact,
)

# NB: Debug output is _not_ transitive over deps, so tsets are not used here.
LinkCommandDebugOutputInfo = provider(
    fields = [
        "debug_outputs",  # ["LinkCommandDebugOutput"]
    ],
)

UnstrippedLinkOutputInfo = provider(
    fields = {
        "artifact": Artifact,
    }
)

def make_link_command_debug_output(linked_object: LinkedObject) -> [LinkCommandDebugOutput, None]:
    if not linked_object.output or not linked_object.linker_command or not linked_object.linker_argsfile:
        return None
    return LinkCommandDebugOutput(
        filename = linked_object.output.short_path,
        command = linked_object.linker_command,
        argsfile = linked_object.linker_argsfile,
    )

# Given a list of `LinkCommandDebugOutput`, it will produce a JSON info file.
# The JSON info file will contain entries for each link command. In addition,
# it will _not_ materialize any inputs to the link command except:
# - linker argfile
def make_link_command_debug_output_json_info(actions: AnalysisActions, debug_outputs: list[LinkCommandDebugOutput]) -> Artifact:
    json_info = []
    associated_artifacts = []
    for debug_output in debug_outputs:
        json_info.append({
            "argsfile": debug_output.argsfile,
            "command": debug_output.command,
            "filename": debug_output.filename,
        })

        # Ensure all argsfile get materialized, as those are needed for debugging
        associated_artifacts.extend(filter(None, [debug_output.argsfile]))

    # Explicitly drop all inputs by using `with_inputs = False`, we don't want
    # to materialize all inputs to the link actions (which includes all object files
    # and possibly other shared libraries).
    json_output = actions.write_json("linker.command", json_info, with_inputs = False, has_content_based_path = False)
    json_output_with_artifacts = json_output.with_associated_artifacts(associated_artifacts)
    return json_output_with_artifacts
