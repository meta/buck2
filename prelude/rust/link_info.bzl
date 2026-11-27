# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Implementation of the Rust build rules.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",
)
load(
    "@prelude//cxx:cxx.bzl",
    "get_auto_link_group_specs",
)
load(
    "@prelude//cxx:cxx_library_utility.bzl",
    "cxx_attr_deps",
    "cxx_is_gnu",
)
load(
    "@prelude//cxx:cxx_link_utility.bzl",
    "ExecutableSharedLibArguments",  # @unused Used as a type
    "executable_shared_lib_arguments_template",
)
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "CxxToolchainInfo",  # @unused Used as a type
    "PicBehavior",
)
load(
    "@prelude//cxx:link_groups.bzl",
    "BuildLinkGroupsContext",
    "LinkGroupLinkInfo",  # @unused Used as a type
    "collect_linkables",
    "create_link_groups",
    "get_filtered_labels_to_links_map",
    "get_filtered_links",
    "get_filtered_targets",
    "get_link_group",
    "get_link_group_info",
    "get_link_group_preferred_linkage",
    "get_public_link_group_nodes",
)
load(
    "@prelude//cxx:link_groups_types.bzl",
    "LinkGroupInfo",  # @unused Used as a type
)
load(
    "@prelude//cxx:transformation_spec.bzl",
    "TransformationSpecContext",  # @unused Used as a type
)
load(
    "@prelude//linking:link_groups.bzl",
    "LinkGroupLib",  # @unused Used as a type
    "LinkGroupLibInfo",  # @unused Used as a type
)
load(
    "@prelude//linking:link_info.bzl",
    "LibOutputStyle",
    "LinkInfo",
    "LinkStrategy",
    "MergedLinkInfo",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableGraph",
    "create_linkable_graph",
    "reduce_linkable_graph",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "traverse_shared_library_info",
)
load(
    "@prelude//linking:types.bzl",
    "Linkage",  # @unused Used as a type
)
load(
    "@prelude//rust/tools:attrs.bzl",
    "RustInternalToolsInfo",  # @unused Used as a type
)
load("@prelude//third-party:providers.bzl", "ThirdPartyBuildInfo")
load(
    "@prelude//utils:type_defs.bzl",
    "is_dict",
    "is_string",
)
load(
    ":build_params.bzl",
    "MetadataKind",  # @unused Used as a type
)
load(
    ":crate_name.bzl",
    "CrateName",  # @unused Used as a type
)
load(
    ":dep_context.bzl",
    "DepCollectionContext",  # @unused Used as a type
)
load(":rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo")

# Link strategy for targets which do not set an explicit `link_style` attribute.
#
# These values are also used as the defaults for check/clippy subtargets on
# libraries, and are the only way in which metadata-fast output can be built.
#
# Internally at Meta, these are a good choice for a default because they allow
# sharing work between check builds and dev mode builds, which have shared link
# strategy, and so consume their dependencies as `static_pic`.
DEFAULT_STATIC_LINK_STRATEGY = LinkStrategy("static_pic")
DEFAULT_STATIC_LIB_OUTPUT_STYLE = LibOutputStyle("pic_archive")

RustProcMacroPlugin = plugins.kind()

# Provided by `rust_proc_macro_alias`, which is how a proc macro appears among a crate's deps. It
# only carries the `target_label` of the proc macro; the proc macro's own providers are accessed
# through `ctx.plugins` (see `get_available_proc_macros`).
RustProcMacroMarker = provider(
    fields = {
        "label": typing.Any,
    }
)

# Artifact produced by a Rust compiler invocation.
#
# The artifact is assigned a provisional filepath at analysis time. However, the
# real name of the crate may not be known until build time. For loading this
# artifact as a transitive dependency, the filename matters and must match the
# real crate name. We keep track of the crate name in its own artifact and use
# it to create a symlink to make the Rust artifact appear with the required name
# to later rustc executions.
RustArtifact = record(
    artifact = field(Artifact),
    crate = field(CrateName),
)

# A proc macro, in the places where a library would provide `RustLinkInfo`. rustc loads proc
# macros rather than linking them into their dependents, so this carries none of a library's link
# providers, and there is no per-link-strategy information because proc macros are always built
# the same way.
RustProcMacroInfo = provider(
    fields = {
        "crate": CrateName,
        "dylib": Artifact,
    },
)

RustProcMacroArchVariant = record(
    dylib = field(Artifact),
)

RustProcMacroArchInfo = provider(
    fields = {
        "arch_variants": dict[str, RustProcMacroArchVariant],
        "crate": CrateName,
    },
)

# Information which is keyed on link_style
RustLinkStrategyInfo = record(
    # Path to the rlib, rmeta, dylib, etc.
    outputs = field(dict[MetadataKind, Artifact]),
    # The full metadata of an `outputs` rlib that only embeds a metadata stub
    # (rustc's `-Zembed-metadata=no`). rustc needs it passed as a second
    # `--extern` for the same crate.
    split_metadata = field(dict[MetadataKind, Artifact] | None, None),
    # Rustc-generated debug info which is referenced -- but not included -- by the
    # linkable rlib. Does not include external debug info from non-Rust native deps.
    #
    # In the `advanced_unstable_linking` case, this is left empty; split debug info is just
    # tracked in the native link providers like it usually would be.
    rust_debug_info = ArtifactTSet | None,
)

# A Rust library, as it is on its node of a `RustGraph`.
RustCrate = record(
    # The crate's `-Cmetadata`, which is what rustc and the linker know it by. See
    # `crate_metadata`.
    metadata = field(str),
    # The target it is built from.
    label = field(ConfiguredTargetLabel),
    crate = field(CrateName),
    # `RustLinkStrategyInfo.outputs` of each link strategy.
    outputs = field(dict[LinkStrategy, dict[MetadataKind, Artifact]]),
    # The crate's sources. They are inputs of every compile that depends on the crate, for crate
    # hashes that are the same wherever the compile runs
    # (https://github.com/rust-lang/rust/issues/153898): rustc desugars a format string
    # differently depending on whether it can read the string back from the file it is in, and a
    # `macro_rules!` macro of a dependency expands to strings in that dependency's files. A proc
    # macro has no way to expand to a string that reads back from its own files, or from those
    # of what it is built from, so those are not inputs. `test_rust_build_reproducibility`
    # covers both kinds of macro.
    srcs = field(Artifact),
    # The proc macros among the crate's dependencies, as keys into `get_available_proc_macros`.
    # A dependent has to load them too, and each dependent gets them from its own plugins.
    proc_macro_deps = field(list[TargetLabel]),
)

# One node of a `RustGraph`.
RustGraphNode = record(
    # None on a node that only has link providers. The native dependencies of a crate are on such
    # a node.
    crate = field(RustCrate | None, None),
    # The native link providers that the node gives to every Rust crate that reaches it. On the
    # node of a crate these are the crate's own, if the crate takes part in the links of its Rust
    # dependents as itself (see `RustLinkInfo.graph`).
    merged_link_infos = field(list[(ConfiguredTargetLabel, MergedLinkInfo)], []),
    linkable_graphs = field(list[LinkableGraph], []),
    # Rust currently treats all native dependencies as being exported, in the sense of C++
    # `exported_deps`. However, they are not only exported from the Rust library that directly
    # depends on them, they are also exported through any further chains of Rust libraries. This
    # list tracks those dependencies.
    #
    # FIXME(JakobDegen): We should not default to treating all native deps as exported.
    exported_deps = field(list[Dependency], []),
)

# The projections of `RustGraph` for one choice of each crate's artifact.
CrateArtifactsProjection = record(
    # Projects a crate to the artifact and to `RustCrate.srcs`, which is everything of the crate
    # that a compile against the artifact has to have as an input. Sources are in the same
    # projection as the artifacts so that an action takes one projection: the input directories
    # of two projections over the same crates are merged again for every action.
    args = field(str),
    # Projects a crate to what `transitive_dependency_symlinks.py` needs to know about the
    # artifact, or to `null` if the crate has no such artifact.
    json = field(str),
)

def _crate_artifacts_projection(link_strategy: LinkStrategy, metadata_kind: MetadataKind) -> CrateArtifactsProjection:
    name = "{}-{}".format(link_strategy.value, metadata_kind.value)
    return CrateArtifactsProjection(args = name, json = name + "-json")

def _project_artifact_args(link_strategy: LinkStrategy, metadata_kind: MetadataKind) -> typing.Callable[[RustGraphNode], list[Artifact]]:
    def project(node: RustGraphNode) -> list[Artifact]:
        crate = node.crate
        if crate == None:
            return []
        artifact = crate.outputs[link_strategy].get(metadata_kind)
        return [crate.srcs, artifact] if artifact else [crate.srcs]

    return project

def _project_artifact_json(link_strategy: LinkStrategy, metadata_kind: MetadataKind) -> typing.Callable[[RustGraphNode], (Artifact, Artifact | None) | None]:
    def project(node: RustGraphNode) -> (Artifact, Artifact | None) | None:
        crate = node.crate
        artifact = crate.outputs[link_strategy].get(metadata_kind) if crate else None
        return (artifact, crate.crate.dynamic) if artifact else None

    return project

def _project_dynamic_name_args(node: RustGraphNode) -> Artifact | cmd_args:
    return (node.crate.crate.dynamic if node.crate else None) or cmd_args()

# Set of RustGraphNode: Rust libraries, each with what its dependents compile against and what
# they link.
#
# Proc macros are not nodes: rustc loads them rather than linking them, and they have no
# transitive dependencies that matter to a dependent.
#
# The postorder is the link order: a crate's native dependencies, which are a node of their own
# ahead of its Rust dependencies, then those, then the crate itself. There are links that only
# work in this order: a C++ library can refer back into the Rust crate that depends on it without
# declaring that (cxx bridges do), which only resolves if the crate comes later.
#
# A projection cannot be given the link strategy and metadata kind that a dependent wants its
# dependencies' artifacts in, so there is a pair of projections for each combination. Get their
# names from `crate_artifacts_projection`.
RustGraph = transitive_set(
    args_projections = {
        _crate_artifacts_projection(link_strategy, metadata_kind).args: _project_artifact_args(link_strategy, metadata_kind)
        for link_strategy in LinkStrategy
        for metadata_kind in MetadataKind
    }
    | {"dynamic_names": _project_dynamic_name_args},
    json_projections = {
        _crate_artifacts_projection(link_strategy, metadata_kind).json: _project_artifact_json(link_strategy, metadata_kind)
        for link_strategy in LinkStrategy
        for metadata_kind in MetadataKind
    },
)

def graph_crates(graph: RustGraph) -> list[RustCrate]:
    """The crates in `graph`."""
    return [node.crate for node in graph.traverse() if node.crate]

# Output of a Rust compilation
RustLinkInfo = provider(
    # @unsorted-dict-items
    fields = {
        # crate - crate name
        "crate": CrateName,
        # strategies - information about each LinkStrategy as RustLinkStrategyInfo
        "strategies": dict[LinkStrategy, RustLinkStrategyInfo],
        # This crate and its transitive Rust dependencies: what Rust dependents compile against,
        # which `graph_crates` reads, and the native link providers that they inherit.
        #
        # In what `rust_library` provides, the root is the crate's node, so the children of this
        # set are the crate's dependencies without the crate. A rule that puts a graph together
        # itself may not keep to that.
        #
        # Rust interacts with the native link graph in a non-standard way.
        #
        # The first difference is in the re-export behavior of Rust compared to C++. The native link
        # providers make an assumption that if one node in the link graph references a symbol in
        # another node in the link graph, there is also a corresponding edge in the link graph.
        # Specifically, the first node must declare a direct dependency on the second, a transitive
        # dependency is not enough. For C++, this just means that each library depends in the link
        # graph on its direct deps and their exported deps.
        #
        # For Rust, the situation is different. Because of re-exports and generics causing delayed
        # codegen, the generated object files for a Rust library can generate symbol references to
        # any of the library's transitive Rust dependencies, as well as to the immediate C++
        # dependencies of those libraries. So to account for that, each Rust library reports direct
        # dependencies on all of those libraries in the link graph. `graph` holds the providers
        # from all of those libraries.
        #
        # The second difference is unique to the case where `advanced_unstable_linking` is not set
        # on the toolchain. Imagine we have a Rust library `:B` with its only one dependency `:A`,
        # another Rust library. The Rust rules give Rust -> Rust dependencies special treatment in
        # the non-`advanced_unstable_linking` case. As a result, the `MergedLinkInfo` provided from
        # `:B` is not a "superset" of the `MergedLinkInfo` provided from `:A` (concrete differences
        # discussed below).
        #
        # This distinction is implemented by effectively having each Rust library provide two sets
        # of link providers. The first is the link providers used across Rust -> Rust dependency
        # edges - this is what `graph` and the fields below are. The second set is the one that is
        # used by C++ and other non-Rust dependents, and is returned from the rule like normal. The
        # second set is a superset of the first, that is it includes anything that the first link
        # providers added.
        #
        # The concrete difference is that the Rust `MergedLinkInfo` provided by `:A` is only the
        # result of merging the `MergedLinkInfo`s from `:A`'s deps, and does not contain anything
        # about `:A`. Instead, when `:B` produces the native `MergedLinkInfo`, it will add a single
        # static library that bundles all transitive Rust deps, including `:A` (and similarly for
        # the DSO case).
        #
        # With `advanced_unstable_linking`, Rust libraries essentially behave just like C++
        # libraries in the link graph, with the handling of transitive dependencies being the only
        # difference.
        "graph": RustGraph,
        "shared_libs": SharedLibraryInfo,
        "third_party_build_info": ThirdPartyBuildInfo,
        # LinkGroupLibInfo intentionally omitted because the Rust -> Rust version
        # never needs to be different from the Rust -> native version
    },
)

def strategy_info_link_strategy(toolchain_info: RustToolchainInfo, dep_link_strategy: LinkStrategy) -> LinkStrategy:
    """The key of `RustLinkInfo.strategies` and `RustCrate.outputs` that `strategy_info` reads."""
    if dep_link_strategy == LinkStrategy("shared") and not toolchain_info.advanced_unstable_linking:
        return DEFAULT_STATIC_LINK_STRATEGY
    else:
        return dep_link_strategy

def strategy_info(toolchain_info: RustToolchainInfo, info: RustLinkInfo, dep_link_strategy: LinkStrategy) -> RustLinkStrategyInfo:
    return info.strategies[strategy_info_link_strategy(toolchain_info, dep_link_strategy)]

def crate_artifacts_projection(toolchain_info: RustToolchainInfo, dep_link_strategy: LinkStrategy, metadata_kind: MetadataKind) -> CrateArtifactsProjection:
    """
    The projections of `RustGraph` for the artifact that is
    `strategy_info(...).outputs[metadata_kind]` of each crate.
    """
    return _crate_artifacts_projection(strategy_info_link_strategy(toolchain_info, dep_link_strategy), metadata_kind)

# Any dependency of a Rust crate
RustOrNativeDependency = record(
    # The actual dependency
    dep = field(Dependency),
    # The local name, if any (for `named_deps`)
    name = field(None | str | ResolvedStringWithMacros),
    # Any flags for the dependency (`flagged_deps`), which are passed on to rustc.
    flags = field(list[str]),
)

# A Rust crate among a crate's dependencies: a library or a proc macro, told apart by the type of
# `info`.
RustDependency = record(
    info = field(RustLinkInfo | RustProcMacroInfo | RustProcMacroArchInfo),
    # For a proc macro, its entry in `ctx.plugins`, so `dep.label` is its key in
    # `get_available_proc_macros`.
    dep = field(Dependency),
    name = field(None | str | ResolvedStringWithMacros),
    flags = field(list[str]),
)

# Information about cxx link groups that rust depends on
RustCxxLinkGroupInfo = record(
    # cxx link infos to link against
    filtered_links = field(list[LinkInfo]),
    # symbol files args to ensure we export the required symbols
    symbol_files_info = field(LinkInfo),
    # targets to link against
    filtered_targets = field(set[TargetLabel]),
    # information about the link groups
    link_group_info = field(LinkGroupInfo | None),
    # shared libraries created from link groups
    link_group_libs = field(dict[str, LinkGroupLib | None]),
    # mapping from target labels to the corresponding link group link_info
    labels_to_links_map = field(dict[Label, LinkGroupLinkInfo]),
    # Target to link group name where it was actually linked into
    targets_consumed_by_link_groups = field(dict[Label, str]),
    # preferred linkage mode for link group libraries
    link_group_preferred_linkage = field(dict[Label, Linkage]),
)

# Returns all first-order dependencies.
def _do_resolve_deps(
    deps: list[Dependency], named_deps: dict[str, Dependency] | list[(ResolvedStringWithMacros, Dependency)], flagged_deps: list[(Dependency, list[str])] = []
) -> list[RustOrNativeDependency]:
    named_deps_items = named_deps.items() if is_dict(named_deps) else named_deps

    return [
        RustOrNativeDependency(name = name, dep = dep, flags = flags)
        for name, dep, flags in [(None, dep, []) for dep in deps]
        + [(name, dep, []) for name, dep in named_deps_items]
        + [(None, dep, flags) for dep, flags in flagged_deps]
    ]

def gather_explicit_sysroot_deps(dep_ctx: DepCollectionContext) -> list[RustOrNativeDependency]:
    explicit_sysroot_deps = dep_ctx.explicit_sysroot_deps
    if not explicit_sysroot_deps:
        if dep_ctx.advanced_unstable_linking:
            fail("Explicit sysroot deps are required when advanced_unstable_linking is on")
        return []

    out = []
    if explicit_sysroot_deps.core:
        out.append(
            RustOrNativeDependency(
                dep = explicit_sysroot_deps.core,
                name = None,
                flags = ["noprelude", "nounused"],
            )
        )
    if explicit_sysroot_deps.std:
        out.append(
            RustOrNativeDependency(
                dep = explicit_sysroot_deps.std,
                name = None,
                # "force" is used here in order to link std internals (like alloc hooks) even if the rest
                # of std is otherwise unused (e.g. we are building a no_std crate that needs to link with
                # other std-enabled crates as a standalone dylib).
                flags = ["noprelude", "nounused", "force"],
            )
        )
    if explicit_sysroot_deps.proc_macro:
        out.append(
            RustOrNativeDependency(
                dep = explicit_sysroot_deps.proc_macro,
                name = None,
                flags = ["noprelude", "nounused"],
            )
        )

    # When advanced_unstable_linking is on, we only add the dep that matches the
    # panic runtime, and neither for immediate-abort, which links no runtime.
    # Without advanced_unstable_linking, we just let rustc deal with it
    if explicit_sysroot_deps.panic_unwind:
        if not dep_ctx.advanced_unstable_linking or dep_ctx.panic_runtime == PanicRuntime("unwind"):
            out.append(
                RustOrNativeDependency(
                    dep = explicit_sysroot_deps.panic_unwind,
                    name = None,
                    flags = ["noprelude", "nounused"],
                )
            )
    if explicit_sysroot_deps.panic_abort:
        if not dep_ctx.advanced_unstable_linking or dep_ctx.panic_runtime == PanicRuntime("abort"):
            out.append(
                RustOrNativeDependency(
                    dep = explicit_sysroot_deps.panic_abort,
                    name = None,
                    flags = ["noprelude", "nounused"],
                )
            )
    for d in filter(None, [explicit_sysroot_deps.alloc, explicit_sysroot_deps.compiler_builtins]) + explicit_sysroot_deps.others:
        out.append(
            RustOrNativeDependency(
                dep = d,
                name = None,
                flags = ["noprelude", "nounused"],
            )
        )
    return out

# The deps as listed in the target's attrs, so a proc macro appears as its `rust_proc_macro_alias`,
# whose providers are not the proc macro's. Read dependencies' providers through `resolve_deps`.
def resolve_listed_deps(ctx: AnalysisContext, dep_ctx: DepCollectionContext) -> list[RustOrNativeDependency]:
    dependencies = _do_resolve_deps(
        deps = cxx_attr_deps(ctx) + dep_ctx.linker_deps,
        named_deps = ctx.attrs.named_deps,
        flagged_deps = ctx.attrs.flagged_deps,
    )

    if dep_ctx.include_doc_deps:
        dependencies.extend(
            _do_resolve_deps(
                deps = getattr(ctx.attrs, "doc_deps", []),
                named_deps = getattr(ctx.attrs, "doc_named_deps", {}),
            )
        )

    return dependencies + gather_explicit_sysroot_deps(dep_ctx)

def resolve_deps(ctx: AnalysisContext, dep_ctx: DepCollectionContext) -> list[RustOrNativeDependency]:
    available_proc_macros = get_available_proc_macros(ctx)
    out = []
    for dep in resolve_listed_deps(ctx, dep_ctx):
        proc_macro_marker = dep.dep.get(RustProcMacroMarker)
        if proc_macro_marker != None:
            proc_macro = available_proc_macros[proc_macro_marker.label]
            dep = RustOrNativeDependency(
                dep = proc_macro,
                name = dep.name,
                flags = dep.flags,
            )
        elif RustProcMacroInfo in dep.dep:
            # Only the alias propagates the proc macro as a plugin, which is
            # what makes it available to this crate's transitive dependents.
            fail("{}: proc macro `{}` must be depended on through its `rust_proc_macro_alias` target".format(ctx.label, dep.dep.label))
        out.append(dep)
    return out

def resolve_rust_deps_inner(all_deps: list[RustOrNativeDependency]) -> list[RustDependency]:
    rust_deps = []
    for dep in all_deps:
        info = dep.dep.get(RustProcMacroInfo) or dep.dep.get(RustProcMacroArchInfo) or dep.dep.get(RustLinkInfo)
        if info == None:
            continue

        rust_deps.append(
            RustDependency(
                info = info,
                dep = dep.dep,
                name = dep.name,
                flags = dep.flags,
            )
        )
    return rust_deps

def resolve_rust_deps(ctx: AnalysisContext, dep_ctx: DepCollectionContext) -> list[RustDependency]:
    all_deps = resolve_deps(ctx, dep_ctx)
    return resolve_rust_deps_inner(all_deps)

def get_available_proc_macros(ctx: AnalysisContext) -> dict[TargetLabel, Dependency]:
    return {x.label.raw_target(): x for x in ctx.plugins[RustProcMacroPlugin]}

# Returns native link dependencies.
def _native_link_dependencies(ctx: AnalysisContext, dep_ctx: DepCollectionContext) -> list[Dependency]:
    """
    Return all first-order native linkable dependencies of all transitive Rust
    libraries.

    This emulates v1's graph walk, where it traverses through Rust libraries
    looking for non-Rust native link infos (and terminating the search there).
    """
    first_order_deps = [dep.dep for dep in resolve_deps(ctx, dep_ctx)]

    return [d for d in first_order_deps if RustLinkInfo not in d and MergedLinkInfo in d]

# Returns the rust link infos for non-proc macro deps.
#
# This is intended to be used to access the Rust -> Rust link providers
def _rust_non_proc_macro_link_infos(ctx: AnalysisContext, dep_ctx: DepCollectionContext) -> list[RustLinkInfo]:
    return [d.info for d in resolve_rust_deps(ctx, dep_ctx) if isinstance(d.info, RustLinkInfo)]

# The `-Cmetadata` of the crate that is compiled from `label`.
#
# This string is the crate's identity: rustc tells two crates of one name apart by it, and it
# goes into the name of every symbol the crate defines. Whatever it leaves out is thereby
# declared not to matter. Two compilations of a target that differ only in such things are the
# same crate, with the same symbol names, and must not meet in one dependency graph (see
# `resolve_crate_deps`).
#
# The configuration is left out: a target is the same crate however it is configured. That is
# what keeps a function's symbol name the same across build modes, platforms and changes to
# either, which profiles that find functions by name (PGO, AutoFDO, BOLT) rely on, and it keeps
# the configuration out of the inputs of every compile, which content-based output paths need.
#
# A target that really is a different crate in each of two configurations, and needs both in one
# graph, has to say so for itself, by adding something of its own to this string. No target
# needs that today, so there is no attribute for it; putting the configuration back in here for
# every target is not the way to get it.
def crate_metadata(label: Label) -> str:
    return str(label.raw_target())

# A crate's dependencies.
RustCrateDeps = record(
    # The Rust ones.
    direct = field(list[RustDependency]),
    # The crate's native dependencies, the libraries among `direct`, and everything below those.
    graph = field(RustGraph),
    # The proc macros among `direct`, and those that the crates in `graph` depend on, as keys
    # into `get_available_proc_macros`.
    proc_macro_deps = field(list[TargetLabel]),
)

def resolve_crate_deps(ctx: AnalysisContext, direct: list[RustDependency], native: list[Dependency] = [], crate: Label | None = None) -> RustCrateDeps:
    """
    Collects what the dependencies of `crate` bring along: `direct` are the Rust
    ones and `native` the ones whose link providers its Rust dependents inherit.

    Fails if two of the crates have the same `-Cmetadata`, `crate` included,
    which is the case for one target in two configurations. Those are two
    compilations that rustc and the linker take for one crate. Both define all
    of its symbols, and nothing makes the definitions agree: the two differ in
    whatever `crate_metadata` leaves out. A link that is given both keeps one
    definition of each symbol and silently drops the other.

    Nothing checks this for C++ libraries, or for Rust crates that only a C++
    target brings together. Here it is an error at the lowest target that
    depends on both.
    """
    children = []
    if native:
        children.append(
            ctx.actions.tset(
                RustGraph,
                value = RustGraphNode(
                    merged_link_infos = [(dep.label.configured_target(), dep[MergedLinkInfo]) for dep in native],
                    linkable_graphs = [dep[LinkableGraph] for dep in native if LinkableGraph in dep],
                    exported_deps = native,
                ),
            )
        )
    children.extend([dep.info.graph for dep in direct if isinstance(dep.info, RustLinkInfo)])
    graph = ctx.actions.tset(RustGraph, children = children)

    seen = {crate_metadata(crate): crate.configured_target()} if crate else {}
    proc_macro_deps = {dep.dep.label.raw_target(): None for dep in direct if not isinstance(dep.info, RustLinkInfo)}
    for node in graph.traverse():
        dep = node.crate
        if dep == None:
            continue
        other = seen.setdefault(dep.metadata, dep.label)
        if other != dep.label:
            target = dep.label.raw_target()
            fail(
                "`{}` depends on the Rust crate `{}` in two configurations:\n  {}\n  {}\n".format(ctx.label, target, other, dep.label)
                + "A target is the same crate in every configuration, so rustc and the linker take the two for one crate, "
                + "but nothing makes what they produce the same. A Rust dependency graph can contain each crate only once. "
                + "To see how the two are reached, run `buck2 cquery \"allpaths('{}', '{}')\"`.".format(ctx.label.raw_target(), target)
            )
        for proc_macro in dep.proc_macro_deps:
            proc_macro_deps[proc_macro] = None

    return RustCrateDeps(
        direct = direct,
        graph = graph,
        proc_macro_deps = proc_macro_deps.keys(),
    )

def crate_deps(ctx: AnalysisContext, dep_ctx: DepCollectionContext) -> RustCrateDeps:
    """The dependencies of the crate being analyzed."""
    return resolve_crate_deps(ctx, resolve_rust_deps(ctx, dep_ctx), _native_link_dependencies(ctx, dep_ctx), crate = ctx.label)

def crate_graph(
    ctx: AnalysisContext,
    deps: RustCrateDeps,
    strategies: dict[LinkStrategy, RustLinkStrategyInfo],
    srcs: Artifact,
    merged_link_info: MergedLinkInfo | None = None,
    linkable_graph: LinkableGraph | None = None,
) -> RustGraph:
    """
    The value of `RustLinkInfo.graph` for the library being analyzed. The link
    providers are the library's own, for a library whose Rust dependents link
    it as itself.
    """
    return ctx.actions.tset(
        RustGraph,
        value = RustGraphNode(
            crate = RustCrate(
                metadata = crate_metadata(ctx.label),
                label = ctx.label.configured_target(),
                crate = attr_crate(ctx),
                outputs = {link_strategy: info.outputs for link_strategy, info in strategies.items()},
                srcs = srcs,
                proc_macro_deps = [dep.dep.label.raw_target() for dep in deps.direct if not isinstance(dep.info, RustLinkInfo)],
            ),
            merged_link_infos = [(ctx.label.configured_target(), merged_link_info)] if merged_link_info else [],
            linkable_graphs = [linkable_graph] if linkable_graph else [],
        ),
        children = [deps.graph],
    )

def inherited_third_party_builds(ctx: AnalysisContext, dep_ctx: DepCollectionContext) -> list[ThirdPartyBuildInfo]:
    infos = []
    infos.extend([d[ThirdPartyBuildInfo] for d in _native_link_dependencies(ctx, dep_ctx) if ThirdPartyBuildInfo in d])
    for dep in _rust_non_proc_macro_link_infos(ctx, dep_ctx):
        infos.append(dep.third_party_build_info)
    return infos

def inherited_rust_cxx_link_group_info(
    ctx: AnalysisContext, link_graphs: list[LinkableGraph], link_strategy: LinkStrategy, transformation_spec_context: TransformationSpecContext | None
) -> RustCxxLinkGroupInfo | None:
    # Check minimum requirements
    if not cxx_is_gnu(ctx) or not ctx.attrs.auto_link_groups:
        return None

    link_group = get_link_group(ctx)

    # Assume a rust executable wants to use link groups if a link group map
    # is present
    link_group_info = get_link_group_info(ctx, link_graphs, link_strategy)
    if link_group_info == None:
        return None

    link_groups = link_group_info.groups
    link_group_mappings = link_group_info.mappings
    link_group_preferred_linkage = get_link_group_preferred_linkage(link_groups.values())

    auto_link_group_specs = get_auto_link_group_specs(ctx, link_group_info)
    linkable_graph = create_linkable_graph(
        ctx,
        deps = link_graphs,
    )
    reduced_linkable_graph = reduce_linkable_graph(linkable_graph)
    linkable_graph_node_map = reduced_linkable_graph.nodes

    executable_deps = []
    for g in link_graphs:
        if g.label in linkable_graph_node_map:
            executable_deps.append(g.label)
        else:
            # handle labels that are mutated by version alias
            executable_deps.append(g.nodes.value.label)

    public_link_group_nodes = get_public_link_group_nodes(
        linkable_graph_node_map,
        link_group_mappings,
        executable_deps,
        link_group,
    )

    linked_link_groups = create_link_groups(
        ctx = ctx,
        link_groups = link_groups,
        link_strategy = link_strategy,
        linkable_graph = reduced_linkable_graph,
        link_group_mappings = link_group_mappings,
        link_group_preferred_linkage = link_group_preferred_linkage,
        executable_deps = executable_deps,
        linker_flags = [],
        link_group_specs = auto_link_group_specs,
        other_roots = [],
        prefer_stripped_objects = False,  # Does Rust ever use stripped objects?
        anonymous = ctx.attrs.anonymous_link_groups,
        public_nodes = public_link_group_nodes,
        transformation_spec_context = transformation_spec_context,
    )

    auto_link_groups = {}
    link_group_libs = {}

    for name, linked_link_group in linked_link_groups.libs.items():
        auto_link_groups[name] = linked_link_group.artifact
        if linked_link_group.library != None:
            link_group_libs[name] = linked_link_group.library

    roots = set(executable_deps)
    pic_behavior = PicBehavior("always_enabled") if link_strategy == LinkStrategy("static_pic") else PicBehavior("supported")
    is_executable_link = True
    exec_linkables = collect_linkables(
        reduced_linkable_graph,
        is_executable_link,
        link_strategy,
        link_group_preferred_linkage,
        pic_behavior,
        roots,
    )
    build_context = BuildLinkGroupsContext(
        public_nodes = public_link_group_nodes,
        linkable_graph = reduced_linkable_graph,
        link_groups = link_groups,
        link_group_mappings = link_group_mappings,
        link_group_preferred_linkage = link_group_preferred_linkage,
        link_strategy = link_strategy,
        pic_behavior = pic_behavior,
        link_group_libs = {name: (lib.label, lib.shared_link_infos) for name, lib in link_group_libs.items()},
        prefer_stripped = False,
        prefer_optimized = False,
    )
    labels_to_links = get_filtered_labels_to_links_map(
        link_group = link_group,
        linkables = exec_linkables,
        is_executable_link = is_executable_link,
        build_context = build_context,
        force_static_follows_dependents = True,
    )

    return RustCxxLinkGroupInfo(
        filtered_links = get_filtered_links(labels_to_links.map),
        symbol_files_info = LinkInfo(
            pre_flags = linked_link_groups.symbol_ldflags,
        ),
        filtered_targets = get_filtered_targets(labels_to_links.map),
        link_group_info = link_group_info,
        link_group_libs = link_group_libs,
        labels_to_links_map = labels_to_links.map,
        targets_consumed_by_link_groups = linked_link_groups.targets_consumed_by_link_groups,
        link_group_preferred_linkage = link_group_preferred_linkage,
    )

# The native link providers that reach a crate through its dependencies: those of its own native
# dependencies, and everything in the `graph`s of its Rust dependencies.
RustInheritedLinkDeps = record(
    # In link order, with one entry per label.
    merged_link_infos = field(list[MergedLinkInfo]),
    linkable_graphs = field(list[LinkableGraph]),
    exported_deps = field(list[Dependency]),
)

def _inconsistent_link_deps(label: typing.Any, a: typing.Any, b: typing.Any):
    fail("cannot depend on {} in multiple inconsistent ways:\n{}\n{}".format(label, a, b))

def inherited_link_deps(deps: RustCrateDeps) -> RustInheritedLinkDeps:
    merged_link_infos = {}
    linkable_graphs = {}
    exported_deps = {}
    for node in deps.graph.traverse(ordering = "postorder"):
        for label, merged_link_info in node.merged_link_infos:
            other = merged_link_infos.get(label)
            if other == None:
                merged_link_infos[label] = merged_link_info
            elif other != merged_link_info:
                _inconsistent_link_deps(label, other, merged_link_info)
        for linkable_graph in node.linkable_graphs:
            other = linkable_graphs.get(linkable_graph.label)
            if other == None:
                linkable_graphs[linkable_graph.label] = linkable_graph
            elif other != linkable_graph:
                _inconsistent_link_deps(linkable_graph.label, other, linkable_graph)
        for dep in node.exported_deps:
            other = exported_deps.get(dep.label)
            if other == None:
                exported_deps[dep.label] = dep
            elif other != dep:
                _inconsistent_link_deps(dep.label, other, dep)

    return RustInheritedLinkDeps(
        merged_link_infos = merged_link_infos.values(),
        linkable_graphs = linkable_graphs.values(),
        exported_deps = exported_deps.values(),
    )

def inherited_shared_libs(ctx: AnalysisContext, dep_ctx: DepCollectionContext) -> list[SharedLibraryInfo]:
    infos = []
    infos.extend([d[SharedLibraryInfo] for d in _native_link_dependencies(ctx, dep_ctx)])
    infos.extend([d.shared_libs for d in _rust_non_proc_macro_link_infos(ctx, dep_ctx)])
    return infos

def inherited_link_group_lib_infos(ctx: AnalysisContext, dep_ctx: DepCollectionContext) -> list[LinkGroupLibInfo]:
    # There are no special Rust -> Rust versions of this provider
    deps = {}
    for d in resolve_deps(ctx, dep_ctx):
        i = d.dep.get(LinkGroupLibInfo)
        if i:
            deps[d.dep.label] = i
    return deps.values()

def inherited_rust_external_debug_info(ctx: AnalysisContext, dep_ctx: DepCollectionContext, link_strategy: LinkStrategy) -> list[ArtifactTSet]:
    toolchain_info = ctx.attrs._rust_toolchain[RustToolchainInfo]
    return filter(None, [strategy_info(toolchain_info, info, link_strategy).rust_debug_info for info in _rust_non_proc_macro_link_infos(ctx, dep_ctx)])

def normalize_crate(label: str | ResolvedStringWithMacros) -> str | ResolvedStringWithMacros:
    return label.replace("-", "_") if is_string(label) else label

def attr_simple_crate_for_filenames(ctx: AnalysisContext) -> str:
    """
    A "good enough" identifier to use in filenames. Buck wants to have filenames
    of artifacts figured out before we begin building them. Normally we want a
    crate foo to produce artifact libfoo.rlib; but if crate_dynamic is being
    used, the true crate name is not known until later. In this situation we use
    the rule's name in place of the true crate name in filenames.

    # produces libordinary.rlib
    rust_library(
        name = "ordinary",
        crate = "ordinary",
    )

    # produces libthrift_generated.rlib
    rust_library(
        name = "thrift-generated",
        crate_dynamic = ":get-namespace-from-thrift-file",
    )
    """
    return normalize_crate(ctx.attrs.crate or ctx.label.name)

def attr_crate(ctx: AnalysisContext) -> CrateName:
    """
    The true user-facing name of the crate, which may only be known at build
    time, not during analysis.
    """
    dynamic = getattr(ctx.attrs, "crate_dynamic", None)
    if dynamic:
        dynamic = dynamic.get(DefaultInfo).default_outputs[0]
    return CrateName(
        simple = normalize_crate(ctx.attrs.crate or ctx.label.name),
        dynamic = dynamic,
    )

def run_action_shlib_symlink_tree(
    actions: AnalysisActions,
    internal_tools_info: RustInternalToolsInfo,
    shared_library_info: SharedLibraryInfo,
    shared_libs_symlink_tree_name_arg: str,
    dwp_symlink_tree_name_arg: str,
) -> (Artifact, Artifact):
    """Runs an action that creates 2 shared library symlink trees from a `SharedLibraryInfo` (i.e., a `TransitiveSet`).

    Returns:
      A pair of artifacts that it creates.  The 1st is the shared library symlink tree.  The 2nd is the dwp symlink tree.
    """
    shared_libs_symlink_tree = actions.declare_output(shared_libs_symlink_tree_name_arg, has_content_based_path = False)
    dwp_symlink_tree = actions.declare_output(dwp_symlink_tree_name_arg, has_content_based_path = False)

    shared_library_info_json = actions.write_json(
        "shared_library_info.json",
        shared_library_info.set.project_as_json("symlink_tree"),
        with_inputs = True,
        has_content_based_path = False,
    )

    actions.run(
        [
            internal_tools_info.shared_libraries_symlink_tree,
            cmd_args(shared_libs_symlink_tree.as_output(), format = "--shared_libs_symlink_tree={}"),
            cmd_args(dwp_symlink_tree.as_output(), format = "--dwp_symlink_tree={}"),
            cmd_args(shared_library_info_json, format = "--shared_libraries_info_json={}"),
        ],
        category = "rust_shared_library_symlinks",
    )
    return (shared_libs_symlink_tree, dwp_symlink_tree)

def executable_shared_lib_arguments_from_shared_library_info(
    ctx: AnalysisContext, cxx_toolchain: CxxToolchainInfo, internal_tools_info: RustInternalToolsInfo, output: Artifact, shared_library_info: SharedLibraryInfo
) -> ExecutableSharedLibArguments:
    """A version of `prelude/cxx/cxx_link_utilit.bzl#executable_shared_lib_arguments`
    that uses the `TransitiveSet` properties of `SharedLibraryInfo` to save memory and
    runtime costs of `TransitiveSet#traversal` calls.
    """

    def create_external_debug_info() -> list[TransitiveSetArgsProjection]:
        if shared_library_info and shared_library_info.set:
            external_debug_info = shared_library_info.set.project_as_args("external_debug_info")
            return [external_debug_info]
        else:
            return []

    def create_shared_libs_symlink_tree_windows() -> list[Artifact]:
        shared_libs = traverse_shared_library_info(shared_library_info, transformation_provider = None)
        return [
            ctx.actions.symlink_file(
                shlib.lib.output.basename,
                shlib.lib.output,
                has_content_based_path = False,
            )
            for shlib in shared_libs
        ]

    def create_shared_libs_symlink_trees(shared_libs_symlink_tree_name_arg: str, dwp_symlink_tree_name_arg: str) -> (Artifact, Artifact) | None:
        if not shared_library_info.set:
            return None

        return run_action_shlib_symlink_tree(
            ctx.actions,
            internal_tools_info,
            shared_library_info,
            shared_libs_symlink_tree_name_arg,
            dwp_symlink_tree_name_arg,
        )

    return executable_shared_lib_arguments_template(
        cxx_toolchain,
        output,
        create_external_debug_info,
        create_shared_libs_symlink_trees,
        create_shared_libs_symlink_tree_windows,
    )
