# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:local_only.bzl", "get_resolved_cxx_binary_link_execution_preference")
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "LinkerType",
    "PicBehavior",
)
load(
    "@prelude//cxx:link.bzl",
    "CxxLinkResult",  # @unused Used as a type
    "cxx_link_shared_library",
)
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference")
load(
    "@prelude//linking:link_info.bzl",
    "LibOutputStyle",
    "LinkArgs",
    "LinkInfo",
    "LinkInfos",
    "LinkStrategy",
    "LinkedObject",
    "SharedLibLinkable",
    "get_lib_output_style",
    "link_info_to_args",
    get_link_info_from_link_infos = "get_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableGraph",  # @unused Used as a type
    "LinkableNode",
    "LinkableRootInfo",
    "get_deps_for_link",
    "get_link_info",
    "get_transitive_deps",
    "linkable_deps",
    "linkable_graph",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibrary",  # @unused Used as a type
    "create_shlib",
)
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//utils:expect.bzl", "expect")
load(
    "@prelude//utils:graph_utils.bzl",
    "depth_first_traversal_by",
    "post_order_traversal",
)
load("@prelude//utils:utils.bzl", "flatten", "value_or")
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(
    ":link_types.bzl",
    "link_options",
)
load(
    ":linker.bzl",
    "get_default_shared_library_name",
    "get_ignore_undefined_symbols_flags",
    "get_no_as_needed_shared_libs_flags",
    "get_shared_library_name",
    "sandbox_exported_linker_flags",
)
load(
    ":symbols.bzl",
    "create_global_symbols_version_script",
    "extract_global_syms",
    "extract_symbol_names",
    "extract_undefined_syms",
    "get_undefined_symbols_args",
)

OmnibusEnvironment = provider(
    # @unsorted-dict-items
    fields = {
        "dummy_omnibus": provider_field(typing.Any, default = None),
        "exclusions": provider_field(typing.Any, default = None),
        "roots": provider_field(typing.Any, default = None),
        "enable_explicit_roots": provider_field(typing.Any, default = None),
        "prefer_stripped_objects": provider_field(typing.Any, default = None),
        "shared_root_ld_flags": provider_field(typing.Any, default = None),
        "force_hybrid_links": provider_field(typing.Any, default = None),
    },
)

Disposition = enum("root", "excluded", "body", "omitted")

OmnibusGraph = record(
    nodes = field(dict[Label, LinkableNode]),
    # All potential root notes for an omnibus link (e.g. C++ libraries,
    # C++ Python extensions).
    roots = field(dict[Label, LinkableRootInfo]),
    # All nodes that should be excluded from libomnibus.
    excluded = field(dict[Label, None]),
)

# Bookkeeping information used to setup omnibus link rules.
OmnibusSpec = record(
    body = field(dict[Label, None], {}),
    excluded = field(dict[Label, None], {}),
    roots = field(dict[Label, LinkableRootInfo], {}),
    exclusion_roots = field(list[Label]),
    # All link infos.
    link_infos = field(dict[Label, LinkableNode], {}),
    dispositions = field(dict[Label, Disposition]),
)

OmnibusPrivateRootProductCause = record(
    category = field(str),
    # Miss-assigned label
    label = field([Label, None], default = None),
    # Its actual disposiiton
    disposition = field([Disposition, None], default = None),
)

OmnibusRootProduct = record(
    shared_library = field(LinkedObject),
    undefined_syms = field(Artifact),
    global_syms = field(Artifact),
)

# The result of the omnibus link.
OmnibusSharedLibraries = record(
    omnibus = field([CxxLinkResult, None], None),
    libraries = field(list[SharedLibrary], []),
    roots = field(dict[Label, OmnibusRootProduct], {}),
    exclusion_roots = field(list[Label]),
    excluded = field(list[Label]),
    dispositions = field(dict[Label, Disposition]),
)

def get_omnibus_graph(graph: LinkableGraph, roots: dict[Label, LinkableRootInfo], excluded: dict[Label, None]) -> OmnibusGraph:
    graph_nodes = graph.nodes.traverse()
    nodes = {}
    for node in filter(None, graph_nodes):
        if node.linkable:
            nodes[node.label] = node.linkable
        roots.update(node.roots)
        excluded.update(node.excluded)
    return OmnibusGraph(nodes = nodes, roots = roots, excluded = excluded)

def get_roots(deps: list[Dependency]) -> dict[Label, LinkableRootInfo]:
    roots = {}
    for dep in deps:
        if LinkableRootInfo in dep:
            root = dep[LinkableRootInfo]
            roots[root.label] = root
    return roots

def get_excluded(deps: list[Dependency] = []) -> dict[Label, None]:
    excluded_nodes = {}
    for dep in deps:
        dep_info = linkable_graph(dep)
        if dep_info != None:
            excluded_nodes[dep_info.label] = None
    return excluded_nodes

def create_linkable_root(label: Label, link_infos: LinkInfos, name: [str, None] = None, deps: list[LinkableGraph | Dependency] = []) -> LinkableRootInfo:
    # Only include dependencies that are linkable.
    return LinkableRootInfo(
        label = label,
        name = name,
        link_infos = link_infos,
        deps = linkable_deps(deps),
    )

def _omnibus_soname(ctx, name: str):
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    return get_shared_library_name(linker_info, name, apply_default_prefix = True)

def create_dummy_omnibus(ctx: AnalysisContext, omnibus_lib_name: str, extra_ldflags: list[typing.Any] = [], anonymous: bool = False) -> Artifact:
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    link_result = cxx_link_shared_library(
        ctx = ctx,
        output = get_shared_library_name(linker_info, "omnibus-dummy", apply_default_prefix = True),
        name = _omnibus_soname(ctx, omnibus_lib_name),
        opts = link_options(
            links = [LinkArgs(flags = extra_ldflags)],
            category_suffix = "dummy_omnibus",
            link_execution_preference = LinkExecutionPreference("any"),
            produce_shared_library_interface = False,
        ),
        anonymous = anonymous,
    )
    return link_result.linked_object.output

def _link_deps(link_infos: dict[Label, LinkableNode], deps: list[Label], pic_behavior: PicBehavior) -> list[Label]:
    """
    Return transitive deps required to link dynamically against the given deps.
    This will following through deps of statically linked inputs and exported
    deps of everything else (see https://fburl.com/diffusion/rartsbkw from v1).
    """

    def find_deps(node: Label):
        return get_deps_for_link(link_infos[node], LinkStrategy("shared"), pic_behavior)

    return depth_first_traversal_by(link_infos, deps, find_deps)

def _create_root(
    ctx: AnalysisContext,
    spec: OmnibusSpec,
    root_products: dict[Label, OmnibusRootProduct],
    root: LinkableRootInfo,
    label: Label,
    link_deps: list[Label],
    omnibus: Artifact,
    pic_behavior: PicBehavior,
    extra_ldflags: list[typing.Any] = [],
    prefer_stripped_objects: bool = False,
    allow_cache_upload: bool = False,
    anonymous: bool = False,
    hash_counter = 0,
) -> OmnibusRootProduct:
    """
    Link a root omnibus node.
    """

    toolchain_info = get_cxx_toolchain_info(ctx)
    linker_info = toolchain_info.linker_info
    linker_type = linker_info.type

    inputs = []

    # Since we're linking against a dummy omnibus which has no symbols, we need
    # to make sure the linker won't drop it from the link or complain about
    # missing symbols.
    inputs.append(
        LinkInfo(
            pre_flags = get_ignore_undefined_symbols_flags(linker_type),
        )
    )

    # add native target link input
    inputs.append(
        get_link_info_from_link_infos(
            root.link_infos,
            prefer_stripped = prefer_stripped_objects,
        ),
    )

    # Link to Omnibus
    if spec.body:
        pre_flags, post_flags = sandbox_exported_linker_flags(
            linker_info = linker_info,
            flags = get_no_as_needed_shared_libs_flags(linker_type),
            post_flags = [],
        )
        inputs.append(
            LinkInfo(
                pre_flags = pre_flags,
                linkables = [SharedLibLinkable(lib = omnibus)],
                post_flags = post_flags,
            )
        )

    # Add deps of the root to the link line.
    for dep in link_deps:
        node = spec.link_infos[dep]
        output_style = get_lib_output_style(
            LinkStrategy("shared"),
            node.preferred_linkage,
            pic_behavior,
        )

        # If this dep needs to be linked statically, then link it directly.
        if output_style != LibOutputStyle("shared_lib"):
            inputs.append(
                get_link_info(
                    node,
                    output_style,
                    prefer_stripped = prefer_stripped_objects,
                )
            )
            continue

        # If this is another root.
        if dep in spec.roots:
            other_root = root_products[dep]

            # TODO(cjhopman): This should be passing structured linkables
            inputs.append(LinkInfo(linkables = [SharedLibLinkable(lib = other_root.shared_library.output)]))
            continue

        # If this node is in omnibus, just add that to the link line.
        if dep in spec.body:
            continue

        # At this point, this should definitely be an excluded node.
        expect(dep in spec.excluded, str(dep))

        # We should have already handled statically linked nodes above.
        expect(output_style == LibOutputStyle("shared_lib"))
        inputs.append(get_link_info(node, output_style))

    output = value_or(
        root.name,
        get_default_shared_library_name(
            linker_info,
            label,
        ),
    )

    # link the rule
    link_result = cxx_link_shared_library(
        ctx = ctx,
        output = output,
        name = root.name,
        opts = link_options(
            links = [LinkArgs(flags = extra_ldflags), LinkArgs(infos = inputs)],
            category_suffix = "omnibus_root",
            identifier = root.name or output,
            link_execution_preference = LinkExecutionPreference("any"),
            allow_cache_upload = allow_cache_upload,
            produce_shared_library_interface = False,
        ),
        anonymous = anonymous,
    )
    shared_library = link_result.linked_object

    return OmnibusRootProduct(
        shared_library = shared_library,
        global_syms = extract_global_syms(
            ctx,
            cxx_toolchain = toolchain_info,
            output = shared_library.output,
            category_prefix = "omnibus",
            # Same as above.
            prefer_local = not anonymous,
            allow_cache_upload = allow_cache_upload,
            anonymous = anonymous,
        ),
        undefined_syms = extract_undefined_syms(
            ctx,
            cxx_toolchain = toolchain_info,
            output = shared_library.output,
            # Don't extract weak-undefined symbols, as passing these back into
            # the omnibus link via `-u` causes undefined sym link failures.
            weak = False,
            category_prefix = "omnibus",
            # Same as above.
            prefer_local = not anonymous,
            allow_cache_upload = allow_cache_upload,
            hash_counter = hash_counter,
            anonymous = anonymous,
        ),
    )

def _extract_global_symbols_from_link_args(
    ctx: AnalysisContext, name: str, link_args: list[[Artifact, ResolvedStringWithMacros, cmd_args, str]], prefer_local: bool = False
) -> Artifact:
    """
    Extract global symbols explicitly set in the given linker args (e.g.
    `-Wl,--export-dynamic-symbol=<sym>`).
    """

    # TODO(T110378137): This is ported from D24065414, but it might make sense
    # to explicitly tell Buck about the global symbols, rather than us trying to
    # extract it from linker flags (which is brittle).
    output = ctx.actions.declare_output(name, has_content_based_path = False)

    # We intentionally drop the artifacts referenced in the args when generating
    # the argsfile -- we just want to parse out symbol name flags and don't need
    # to materialize artifacts to do this.
    argsfile, _ = ctx.actions.write(name + ".args", link_args, allow_args = True, has_content_based_path = False)

    # TODO(T110378133): Make this work with other platforms.
    param = "--export-dynamic-symbol"
    pattern = "\\(-Wl,\\)\\?{}[,=]\\([^,]*\\)".format(param)

    # Used sed/grep to filter the symbol name from the relevant flags.
    # TODO(T110378130): As is the case in v1, we don't properly extract flags
    # from argsfiles embedded in existing args.
    script = "set -euo pipefail; " + "cat \"$@\" | (grep -- '{0}' || [[ $? == 1 ]]) | sed 's|{0}|\\2|' | LC_ALL=C sort -S 10% -u > {{}}".format(pattern)
    ctx.actions.run(
        [
            "/usr/bin/env",
            "bash",
            "-c",
            cmd_args(output.as_output(), format = script),
            "",
            argsfile,
        ],
        category = "omnibus_global_symbol_flags",
        prefer_local = prefer_local,
        weight_percentage = 15,  # 10% + a little padding
    )
    return output

def _create_global_symbols_version_script(
    ctx: AnalysisContext, roots: list[OmnibusRootProduct], excluded: list[Artifact], link_args: list[[Artifact, ResolvedStringWithMacros, cmd_args, str]]
) -> Artifact:
    """
    Generate a version script exporting symbols from from the given objects and
    link args.
    """

    # Get global symbols from roots.  We set a rule to do this per-rule, as
    # using a single rule to process all roots adds overhead to the critical
    # path of incremental flows (e.g. that only update a single root).
    global_symbols_files = [root.global_syms for root in roots]

    # TODO(T110378126): Processing all excluded libs together may get expensive.
    # We should probably split this up and operate on individual libs.
    if excluded:
        global_symbols_files.append(
            extract_symbol_names(
                ctx = ctx,
                cxx_toolchain = get_cxx_toolchain_info(ctx),
                name = "__excluded_libs__.global_syms.txt",
                objects = excluded,
                dynamic = True,
                global_only = True,
                category = "omnibus_global_syms_excluded_libs",
            )
        )

    # Extract explicitly globalized symbols from linker args.
    global_symbols_files.append(
        _extract_global_symbols_from_link_args(
            ctx,
            "__global_symbols_from_args__.txt",
            link_args,
        )
    )

    return create_global_symbols_version_script(
        actions = ctx.actions,
        name = "__global_symbols__.vers",
        category = "omnibus_version_script",
        symbol_files = global_symbols_files,
    )

def _is_static_only(info: LinkableNode) -> bool:
    """
    Return whether this can only be linked statically.
    """
    return info.preferred_linkage == Linkage("static")

def _is_shared_only(info: LinkableNode) -> bool:
    """
    Return whether this can only use shared linking
    """
    return info.preferred_linkage == Linkage("shared")

def _is_static_deps(info: LinkableNode) -> bool:
    """
    Return whether avoid excluding this nodes deps, even if this node is shared
    -only.
    """
    return "omnibus_static_deps" in info.labels

def _create_omnibus(
    ctx: AnalysisContext,
    spec: OmnibusSpec,
    omnibus_lib_name: str,
    root_products: dict[Label, OmnibusRootProduct],
    pic_behavior: PicBehavior,
    extra_ldflags: list[typing.Any] = [],
    prefer_stripped_objects: bool = False,
    allow_cache_upload: bool = False,
    enable_distributed_thinlto = False,
) -> CxxLinkResult:
    inputs = []

    # Undefined symbols roots...
    non_body_root_undefined_syms = [root.undefined_syms for label, root in root_products.items() if label not in spec.body]
    if non_body_root_undefined_syms:
        inputs.append(
            LinkInfo(
                pre_flags = [
                    get_undefined_symbols_args(
                        ctx = ctx,
                        name = "__undefined_symbols__.argsfile",
                        symbol_files = non_body_root_undefined_syms,
                        category = "omnibus_undefined_symbols",
                    ),
                ]
            )
        )

    # Process all body nodes.
    deps = {}
    global_symbols_link_args = []
    for label in spec.body:
        # If this body node is a root, add the it's output to the link.
        if label in spec.roots:
            root = root_products[label]

            # TODO(cjhopman): This should be passing structured linkables
            inputs.append(LinkInfo(pre_flags = [cmd_args(root.shared_library.output)]))
            continue

        node = spec.link_infos[label]

        # Otherwise add in the static input for this node.
        output_style = get_lib_output_style(
            LinkStrategy("static_pic"),
            node.preferred_linkage,
            pic_behavior,
        )
        expect(output_style == LibOutputStyle("pic_archive"))
        body_input = get_link_info(
            node,
            output_style,
            prefer_stripped = prefer_stripped_objects,
        )
        inputs.append(body_input)
        global_symbols_link_args.append(link_info_to_args(body_input))

        # Keep track of all first order deps of the omnibus monolith.
        for dep in node.deps + node.exported_deps:
            if dep not in spec.body:
                expect(dep in spec.excluded)
                deps[dep] = None

    toolchain_info = get_cxx_toolchain_info(ctx)

    # Now add deps of omnibus to the link
    for label in _link_deps(spec.link_infos, deps.keys(), toolchain_info.pic_behavior):
        node = spec.link_infos[label]
        output_style = get_lib_output_style(
            LinkStrategy("shared"),
            node.preferred_linkage,
            toolchain_info.pic_behavior,
        )
        inputs.append(
            get_link_info(
                node,
                output_style,
                prefer_stripped = prefer_stripped_objects,
            )
        )

    linker_info = toolchain_info.linker_info

    # Add global symbols version script.
    # FIXME(agallagher): Support global symbols for darwin.
    if linker_info.type != LinkerType("darwin"):
        global_sym_vers = _create_global_symbols_version_script(
            ctx,
            # Extract symbols from roots...
            root_products.values(),
            # ... and the shared libs from excluded nodes.
            [shared_lib.lib.output for label in spec.excluded for shared_lib in spec.link_infos[label].shared_libs.libraries],
            # Extract explicit global symbol names from flags in all body link args.
            global_symbols_link_args,
        )
        inputs.append(
            LinkInfo(
                pre_flags = [
                    "-Wl,--version-script",
                    global_sym_vers,
                    # The version script contains symbols that are not defined. Up to
                    # LLVM 15 this behavior was ignored but LLVM 16 turns it into
                    # warning by default.
                    "-Wl,--undefined-version",
                ]
            )
        )

    soname = _omnibus_soname(ctx, omnibus_lib_name)

    return cxx_link_shared_library(
        ctx = ctx,
        output = soname,
        name = soname,
        opts = link_options(
            links = [LinkArgs(flags = extra_ldflags), LinkArgs(infos = inputs)],
            category_suffix = "omnibus",
            # TODO(T110378138): As with static C++ links, omnibus links are
            # currently too large for RE, so run them locally for now (e.g.
            # https://fb.prod.workplace.com/groups/buck2dev/posts/2953023738319012/).
            # NB: We explicitly pass a value here to override
            # the linker_info.link_libraries_locally that's used by `cxx_link_shared_library`.
            # That's because we do not want to apply the linking behavior universally,
            # just use it for omnibus.
            link_execution_preference = get_resolved_cxx_binary_link_execution_preference(ctx, [], False, toolchain_info),
            link_weight = linker_info.link_weight,
            enable_distributed_thinlto = enable_distributed_thinlto,
            identifier = soname,
            allow_cache_upload = allow_cache_upload,
            produce_shared_library_interface = False,
        ),
    )

def _build_omnibus_spec(ctx: AnalysisContext, graph: OmnibusGraph) -> OmnibusSpec:
    """
    Divide transitive deps into excluded, root, and body nodes, which we'll
    use to link the various parts of omnibus.
    """

    # Build up the set of all nodes that we have to exclude from omnibus linking
    excluded = {}

    # Track excluded nodes that need to transitively exclude their children.
    exclusion_roots = []
    exclusion_roots.extend(graph.excluded.keys())

    # Walk graph nodes, looking for ones to exclude.
    for label, info in graph.nodes.items():
        # Exclude any body nodes which can't be linked statically.
        if (label not in graph.roots) and _is_shared_only(info):
            # By default, we'll also exclude this nodes transitive children,
            # but nodes can opt-out.
            if _is_static_deps(info):
                excluded[label] = None
            else:
                exclusion_roots.append(label)

    # Recursively expand exluded roots and add them to the excluded list.
    for label in get_transitive_deps(
        graph.nodes,
        exclusion_roots,
    ):
        excluded[label] = None

    # Finalized root nodes, after removing any excluded roots.
    roots = {label: root for label, root in graph.roots.items() if label not in excluded}

    # Find the deps of the root nodes that should be linked into
    # 'libomnibus.so'.
    #
    # If a dep indicates preferred linkage static, it is linked directly into
    # this omnimbus root and therefore not added to `first_order_root_deps` and
    # thereby will not be linked into 'libomnibus.so'. If the dep does not
    # indicate preferred linkage static, then it is added to
    # `first_order_root_deps` and thereby will be linked into 'libomnibus.so'.
    first_order_root_deps = []
    for label in _link_deps(graph.nodes, flatten([r.deps for r in roots.values()]), get_cxx_toolchain_info(ctx).pic_behavior):
        # Per the comment above, only consider deps which aren't *only*
        # statically linked.
        if _is_static_only(graph.nodes[label]):
            continue

        # Don't include a root's dep onto another root.
        if label in roots:
            continue
        first_order_root_deps.append(label)

    # All body nodes.  These included all non-excluded body nodes and any non-
    # excluded roots which are reachable by these body nodes (since they will
    # need to be put on the link line).
    body = {label: None for label in get_transitive_deps(graph.nodes, first_order_root_deps) if label not in excluded}

    dispositions = {}

    for node, info in graph.nodes.items():
        if _is_static_only(info):
            continue

        if node in roots:
            dispositions[node] = Disposition("root")
            continue

        if node in excluded:
            dispositions[node] = Disposition("excluded")
            continue

        if node in body:
            dispositions[node] = Disposition("body")
            continue

        # Why does that happen? Who knows with Omnibus :(
        dispositions[node] = Disposition("omitted")

    return OmnibusSpec(
        excluded = excluded,
        roots = roots,
        body = body,
        link_infos = graph.nodes,
        exclusion_roots = exclusion_roots,
        dispositions = dispositions,
    )

def _ordered_roots(spec: OmnibusSpec, pic_behavior: PicBehavior) -> list[(Label, LinkableRootInfo, list[Label])]:
    """
    Return information needed to link the roots nodes.
    """

    # Calculate all deps each root node needs to link against.
    link_deps = {label: _link_deps(spec.link_infos, root.deps, pic_behavior) for label, root in spec.roots.items()}

    # Used the link deps to create the graph of root nodes.
    root_graph = {node: [dep for dep in deps if dep in spec.roots] for node, deps in link_deps.items()}

    # Emit the root link info in post-order, so that we generate root link rules
    # for dependencies before their dependents.
    ordered_roots = [(label, spec.roots[label], link_deps[label]) for label in post_order_traversal(root_graph)]

    return ordered_roots

def create_omnibus_libraries(
    ctx: AnalysisContext,
    graph: OmnibusGraph,
    extra_ldflags: list[typing.Any] = [],
    extra_root_ldflags: dict[Label, list[typing.Any]] = {},
    prefer_stripped_objects: bool = False,
    enable_distributed_thinlto = False,
    omnibus_lib_name: str = "omnibus",
    anonymous: bool = False,
) -> OmnibusSharedLibraries:
    spec = _build_omnibus_spec(ctx, graph)
    pic_behavior = get_cxx_toolchain_info(ctx).pic_behavior

    # Create dummy omnibus
    dummy_omnibus = create_dummy_omnibus(
        ctx = ctx,
        extra_ldflags = extra_ldflags,
        omnibus_lib_name = omnibus_lib_name,
        anonymous = anonymous,
    )

    libraries = []
    root_products = {}
    counter = 0  # counter to avoid hash collisions

    # Link all root nodes against the dummy libomnibus lib.
    for label, root, link_deps in _ordered_roots(spec, pic_behavior):
        counter += 1
        product = _create_root(
            ctx,
            spec,
            root_products,
            root,
            label,
            link_deps,
            dummy_omnibus,
            pic_behavior,
            extra_ldflags + extra_root_ldflags.get(label, []),
            prefer_stripped_objects,
            allow_cache_upload = True,
            hash_counter = counter,
            anonymous = anonymous,
        )
        if root.name != None:
            libraries.append(
                create_shlib(
                    soname = root.name,
                    lib = product.shared_library,
                    label = label,
                ),
            )
        root_products[label] = product

    # If we have body nodes, then link them into the monolithic libomnibus.so.
    omnibus = None
    if spec.body:
        omnibus = _create_omnibus(
            ctx = ctx,
            spec = spec,
            omnibus_lib_name = omnibus_lib_name,
            root_products = root_products,
            pic_behavior = pic_behavior,
            extra_ldflags = extra_ldflags,
            prefer_stripped_objects = prefer_stripped_objects,
            enable_distributed_thinlto = enable_distributed_thinlto,
            allow_cache_upload = True,
        )
        libraries.append(
            create_shlib(
                soname = _omnibus_soname(ctx, omnibus_lib_name),
                lib = omnibus.linked_object,
                label = ctx.label,
            ),
        )

    # For all excluded nodes, just add their regular shared libs.
    for label in spec.excluded:
        libraries.extend(spec.link_infos[label].shared_libs.libraries)

    return OmnibusSharedLibraries(
        omnibus = omnibus,
        libraries = libraries,
        roots = root_products,
        exclusion_roots = spec.exclusion_roots,
        excluded = spec.excluded.keys(),
        dispositions = spec.dispositions,
    )
