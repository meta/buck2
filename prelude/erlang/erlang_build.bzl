# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(":erlang_dependencies.bzl", "fail_dep_conflict")
load(
    ":erlang_info.bzl",
    "DepsMapping",
    "EbinMapping",
    "ErlangAppIncludeInfo",  # @unused Used as type
    "ErlangDependencyInfo",
    "IncludesMapping",
    "PathArtifactMapping",
)
load(":erlang_paths.bzl", "strip_extension")
load(
    ":erlang_toolchain.bzl",
    "Toolchain",  # @unused Used as type
)

_BUILD_DIR = "__build"
_DEP_FILES_DIR = paths.join(_BUILD_DIR, "__dep_files")
_DEP_INFO_FILE = paths.join(_DEP_FILES_DIR, "app.info.dep")
_GENERATED_DIR = paths.join(_BUILD_DIR, "__generated")

BuildEnvironment = record(
    # Entries the target contributes itself. The `dep_` counterparts hold the
    # entries inherited from the dependencies and are shared by reference with
    # the `erlang_deps_rule` that built them, so a target never copies a map
    # sized by its transitive closure.
    includes = field(IncludesMapping, {}),
    include_dirs = field(PathArtifactMapping, {}),
    private_includes = field(IncludesMapping, {}),
    private_include_dirs = field(PathArtifactMapping, {}),
    beams = field(EbinMapping, {}),
    header_deps_files = field(DepsMapping, {}),
    docs = field(dict[str, Artifact], {}),
    dep_includes = field(IncludesMapping, {}),
    dep_include_dirs = field(PathArtifactMapping, {}),
    dep_private_includes = field(IncludesMapping, {}),
    dep_private_include_dirs = field(PathArtifactMapping, {}),
    dep_beams = field(EbinMapping, {}),
    dep_header_deps_files = field(DepsMapping, {}),
    # whether the dependencies' private includes participate in include resolution
    peeked_private_includes = field(bool, False),
)

# The layers a module's dependencies resolve against, built once per application
# rather than per compiled file. Each include layer pairs a mapping with the
# directory mapping it is keyed alongside.
SmallBuildEnvironment = record(
    include_layers = field(list[(IncludesMapping, PathArtifactMapping)]),
    private_layers = field(list[(IncludesMapping, PathArtifactMapping)]),
    beam_layers = field(list[EbinMapping]),
    docs = field(dict[str, Artifact], {}),
)

def _layers(own: dict, dep: dict) -> list[dict]:
    """The maps to consult for a lookup, the target's own entries first."""
    return [own, dep]

def _paired_layers(own: dict, dep: dict, own_dirs: dict, dep_dirs: dict) -> list[(dict, dict)]:
    """`_layers`, pairing each include map with the directory map it is keyed alongside."""
    return [(own, own_dirs), (dep, dep_dirs)]

def _own_or_dep(own: dict, dep: dict, key: str):
    """An application's own entry, which it inherits when a cyclic `_includes_target` contributed it."""
    value = own.get(key, None)
    return value if value != None else dep.get(key, None)

DepInfo = record(
    dep_file = field(Artifact),
    path = field(str),
)

def _prepare_build_environment(
    dep_info: ErlangDependencyInfo, includes_target: [ErlangAppIncludeInfo, None] = None, peek_private_includes: bool = False
) -> BuildEnvironment:
    """Prepare build environment and collect the context from all dependencies."""
    includes = {}
    include_dirs = {}
    header_deps_files = {}

    # it's possible we already depend on our includes target through cyclic
    # extra_includes deps, in which case we don't need to add it again
    if includes_target and not includes_target.name in dep_info.dependencies:
        for file in includes_target.includes:
            for app in dep_info.includes:
                if file in dep_info.includes[app]:
                    fail_dep_conflict("header", file, includes_target.name, app)
            for app in dep_info.private_includes:
                if file in dep_info.private_includes[app]:
                    fail_dep_conflict("header", file, includes_target.name, app)
        include_dirs[includes_target.name] = includes_target.include_dir
        includes[includes_target.name] = includes_target.includes
        header_deps_files[includes_target.name] = includes_target.header_deps_file

    return BuildEnvironment(
        includes = includes,
        include_dirs = include_dirs,
        private_includes = {},
        private_include_dirs = {},
        header_deps_files = header_deps_files,
        beams = {},
        docs = {},
        dep_includes = dep_info.includes,
        dep_include_dirs = dep_info.include_dirs,
        dep_private_includes = dep_info.private_includes,
        dep_private_include_dirs = dep_info.private_include_dirs,
        dep_beams = dep_info.beams,
        dep_header_deps_files = dep_info.header_deps_files,
        peeked_private_includes = peek_private_includes,
    )

def _generated_source_artifacts(ctx: AnalysisContext, toolchain: Toolchain) -> list[Artifact]:
    """Generate source output artifacts and build actions for generated erl files."""

    results = []
    for src in ctx.attrs.srcs:
        if _is_yrl(src):
            results.append(_build_xyrl(ctx, toolchain, src, "yrl_includefile"))
        elif _is_xrl(src):
            results.append(_build_xyrl(ctx, toolchain, src, "xrl_includefile"))
    return results

# mutates build_environment in place
def _generate_include_artifacts(
    ctx: AnalysisContext, toolchain: Toolchain, build_environment: BuildEnvironment, name: str, header_artifacts: list[Artifact], is_private: bool = False
):
    if not header_artifacts:
        return

    include_files = {hrl.basename: hrl for hrl in header_artifacts}
    dir_name = "{}_private".format(name) if is_private else name
    include_dir = ctx.actions.symlinked_dir(paths.join(_BUILD_DIR, dir_name, "include"), include_files, has_content_based_path = False)

    # dep files
    deps_files = _get_deps_files(ctx, toolchain, header_artifacts)

    # detect conflicts
    conflict_layers = _layers(build_environment.includes, build_environment.dep_includes) + _layers(
        build_environment.private_includes, build_environment.dep_private_includes
    )
    for file in deps_files:
        for layer in conflict_layers:
            for app in layer:
                if file in layer[app]:
                    fail_dep_conflict("header", file, name, app)

    previous_merged_file = build_environment.header_deps_files.get(name, None) or build_environment.dep_header_deps_files.get(name, None)
    if previous_merged_file != None:
        build_environment.header_deps_files[name] = _merged_deps_file(ctx, toolchain, name, deps_files, is_private, previous_merged_file)
    else:
        file = _merged_deps_file(ctx, toolchain, name, deps_files, is_private, None)
        if file:
            build_environment.header_deps_files[name] = file

    # construct updates build environment
    if not is_private:
        # fields for public include directory
        build_environment.includes[name] = include_files
        build_environment.include_dirs[name] = include_dir
    else:
        # fields for private include directory
        build_environment.private_includes[name] = include_files
        build_environment.private_include_dirs[name] = include_dir

def _merged_deps_file(
    ctx: AnalysisContext, toolchain: Toolchain, name: str, deps_files: PathArtifactMapping, is_private: bool, previous_merged_file: [Artifact, None]
) -> [Artifact, None]:
    """Merge the deps files of the headers into a single deps file."""

    if not deps_files:
        return previous_merged_file

    name = "{}-private".format(name) if is_private else name

    merged_file = ctx.actions.declare_output(_DEP_FILES_DIR, "{}.merged.dep".format(name), has_content_based_path = False)
    deps_files_json = ctx.actions.write_json(merged_file.short_path + ".json", deps_files, with_inputs = True, has_content_based_path = False)

    cmd = cmd_args(toolchain.dependency_merger, merged_file.as_output(), deps_files_json)
    if previous_merged_file:
        cmd.add(previous_merged_file)

    _run_with_env(
        ctx,
        toolchain,
        cmd,
        category = "dependency_merger",
        identifier = name,
    )

    return merged_file

# mutates build_environment in place
def _generate_beam_artifacts(
    ctx: AnalysisContext, toolchain: Toolchain, build_environment: BuildEnvironment, name: str, src_artifacts: list[Artifact], hermetic_src_dir: Artifact
):
    if not src_artifacts:
        return

    ebin = paths.join(_BUILD_DIR, "ebin")

    beam_mapping = {}
    for erl in src_artifacts:
        module = module_name(erl)
        beam_mapping[module] = ctx.actions.declare_output(ebin, "{}.beam".format(module), has_content_based_path = False)

    # detect conflicts
    for key in beam_mapping:
        for layer in _layers(build_environment.beams, build_environment.dep_beams):
            for app in layer:
                if key in layer[app]:
                    fail_dep_conflict("module", key, name, app)

    build_environment.beams[name] = beam_mapping

    # dep files
    deps_files = _get_deps_files(ctx, toolchain, src_artifacts)
    # the target's own entries first: `dependency_finalizer` keeps the first value it
    # sees for a header, and an application's own merged dep file supersedes the one
    # its `_includes_target` contributed
    dep_info_file = ctx.actions.write_json(
        _DEP_INFO_FILE,
        [build_environment.header_deps_files, build_environment.dep_header_deps_files],
        with_inputs = True,
        has_content_based_path = False,
    )

    peeked = build_environment.peeked_private_includes
    small_build_environment = SmallBuildEnvironment(
        include_layers = _paired_layers(
            build_environment.includes,
            build_environment.dep_includes,
            build_environment.include_dirs,
            build_environment.dep_include_dirs,
        ),
        private_layers = _paired_layers(
            build_environment.private_includes,
            build_environment.dep_private_includes if peeked else {},
            build_environment.private_include_dirs,
            build_environment.dep_private_include_dirs if peeked else {},
        ),
        beam_layers = _layers(build_environment.beams, build_environment.dep_beams),
        docs = build_environment.docs,
    )

    for erl in src_artifacts:
        _build_erl(ctx, toolchain, small_build_environment, deps_files, dep_info_file, hermetic_src_dir, erl, beam_mapping[module_name(erl)])

def _get_deps_files(ctx: AnalysisContext, toolchain: Toolchain, srcs: list[Artifact]):
    """Mapping from the output path to the deps file artifact for each srcs artifact and dependencies."""

    return {src.basename: _get_deps_file(ctx, toolchain, src) for src in srcs}

def _get_deps_file(ctx: AnalysisContext, toolchain: Toolchain, src: Artifact) -> Artifact:
    dependency_json = ctx.actions.declare_output(_DEP_FILES_DIR, "{}.dep".format(src.short_path), has_content_based_path = False)

    _run_with_env(
        ctx,
        toolchain,
        cmd_args(toolchain.dependency_analyzer, src, dependency_json.as_output()),
        category = "dependency_analyzer",
        identifier = src.short_path,
    )
    return dependency_json

def _build_xyrl(ctx: AnalysisContext, toolchain: Toolchain, xyrl: Artifact, custom_include_opt: str) -> Artifact:
    """Generate an erl file out of an xrl or yrl input file."""
    output = ctx.actions.declare_output(_GENERATED_DIR, "{}.erl".format(module_name(xyrl)), has_content_based_path = False)
    erlc = toolchain.otp_binaries.erlc
    custom_include = getattr(ctx.attrs, custom_include_opt, None)
    cmd = cmd_args(erlc)
    if custom_include:
        cmd.add("-I", custom_include)
    cmd.add("-o", cmd_args(output.as_output(), parent = 1), xyrl)
    _run_with_env(
        ctx,
        toolchain,
        cmd,
        error_handler = toolchain.error_handler.erlc,
        category = "erlc",
        identifier = xyrl.basename,
    )
    return output

def _build_erl(
    ctx: AnalysisContext,
    toolchain: Toolchain,
    build_environment: SmallBuildEnvironment,
    beam_deps_files: PathArtifactMapping,
    dep_info_file: WriteJsonCliArgs,
    hermetic_src_dir: Artifact,
    src: Artifact,
    output: Artifact,
) -> None:
    """Compile erl files into beams."""

    final_dep_file = ctx.actions.declare_output(_DEP_FILES_DIR, "{}.final.dep".format(src.short_path), has_content_based_path = False)
    initial_dep_file = beam_deps_files[src.basename]
    _run_with_env(
        ctx,
        toolchain,
        cmd_args(toolchain.dependency_finalizer, initial_dep_file, dep_info_file, final_dep_file.as_output()),
        category = "dependency_finalizer",
        identifier = src.basename,
    )

    # src points to the original file, but erlc will try to find .hrl files on that directory,
    # so we build the version in hermetic_src_dir, that contains symlinks to declared src (we
    # use the `+{source, _}` option to ensure paths still point to the actual src)
    hermetic_src = hermetic_src_dir.project(src.basename)

    def dynamic_lambda(ctx: AnalysisContext, artifacts, outputs):
        erlc = toolchain.otp_binaries.erlc
        erl_opts = _get_erl_opts(ctx, toolchain, src)
        deps_args, mapping = _dependencies_to_args(artifacts, final_dep_file, build_environment)
        erlc_cmd = cmd_args(
            erlc,
            erl_opts,
            deps_args,
            cmd_args(outputs[output].as_output(), parent = 1, format = "-o{}"),
            hermetic_src,
        )
        mapping_file = ctx.actions.write_json(_dep_mapping_name(src), mapping, has_content_based_path = False)
        _run_with_env(
            ctx,
            toolchain,
            erlc_cmd,
            error_handler = toolchain.error_handler.erlc,
            category = "erlc",
            identifier = src.basename,
            env = {"BUCK2_FILE_MAPPING": mapping_file},
            always_print_stderr = True,
        )

    dynamic_inputs = [hermetic_src] + build_environment.docs.values()
    ctx.actions.dynamic_output(dynamic = [final_dep_file], inputs = dynamic_inputs, outputs = [output.as_output()], f = dynamic_lambda)
    return None

def _dependencies_to_args(artifacts, final_dep_file: Artifact, build_environment: SmallBuildEnvironment) -> (cmd_args, dict[str, (bool, [str, Artifact])]):
    """Add the transitive closure of all per-file Erlang dependencies as specified in the deps files to the `args` with .hidden."""
    includes = set()
    include_libs = set()
    beams = set()
    precise_includes = []
    doc_inputs = []

    input_mapping = {}
    deps = artifacts[final_dep_file].read_json()

    include_layers = build_environment.include_layers
    private_layers = build_environment.private_layers

    # silently ignore not found dependencies and let erlc report the not found stuff
    for dep in deps:
        file = dep["file"]
        if dep["type"] == "include_lib":
            app = dep["app"]
            found_app = False
            for app_includes, app_dirs in include_layers:
                if app in app_includes:
                    found_app = True

                    # if file doesn't exist in app, let the compiler fail
                    if file in app_includes[app]:
                        include_dir = app_dirs[app]
                        include_libs.add(include_dir)
                        precise_includes.append(include_dir.project(file))
                        input_mapping[file] = (True, app_includes[app][file])
                    break
            if not found_app:
                # the file might come from OTP
                input_mapping[file] = (False, paths.join(app, "include", file))

        elif dep["type"] == "include":
            # these includes can either reside in the private includes or the public ones
            found_private = False

            # we've checked for duplicates earlier, we'll find at most one
            for app_includes, app_dirs in private_layers:
                for app in app_includes:
                    if file in app_includes[app]:
                        include_dir = app_dirs[app]
                        includes.add(include_dir)
                        precise_includes.append(include_dir.project(file))
                        input_mapping[file] = (True, app_includes[app][file])
                        found_private = True
                        break
                if found_private:
                    break

            if not found_private:
                # we've checked for duplicates earlier, we'll find at most one
                found_public = False
                for app_includes, app_dirs in include_layers:
                    for app in app_includes:
                        if file in app_includes[app]:
                            include_dir = app_dirs[app]
                            includes.add(include_dir)
                            precise_includes.append(include_dir.project(file))
                            input_mapping[file] = (True, app_includes[app][file])
                            found_public = True
                            break
                    if found_public:
                        break

        elif dep["type"] == "behaviour" or dep["type"] == "parse_transform" or dep["type"] == "manual_dependency":
            module = strip_extension(file)

            # we made sure earlier there are no conflicts, we'll find at most one
            found_beam = False
            for layer in build_environment.beam_layers:
                for app in layer:
                    beam = layer[app].get(module, None)
                    if beam != None:
                        beams.add(beam)
                        found_beam = True
                        break
                if found_beam:
                    break

        elif dep["type"] == "doc_file":
            # resolve the annotation path (relative to src/) to a package-relative path
            resolved = paths.normalize(paths.join("src", file))
            if resolved in build_environment.docs:
                doc_inputs.append(build_environment.docs[resolved])

        else:
            fail("unrecognized dependency type %s", (dep["type"]))

    args = cmd_args(
        cmd_args(list(includes), format = "-I{}", ignore_artifacts = True),
        cmd_args(list(include_libs), format = "-I{}", parent = 2, ignore_artifacts = True),
        cmd_args(list(beams), format = "-pa{}", parent = 1),
        hidden = precise_includes + doc_inputs,
    )

    return args, input_mapping

def _get_erl_opts(ctx: AnalysisContext, toolchain: Toolchain, src: Artifact) -> cmd_args:
    always = ["+deterministic"]

    # use erl_opts defined in taret if present
    if getattr(ctx.attrs, "erl_opts", None) == None:
        opts = toolchain.erl_opts
    else:
        opts = ctx.attrs.erl_opts

    # dedupe options
    opts = dedupe(opts + always)

    # build args
    args = cmd_args(opts)

    # add parse_transforms
    parse_transforms = dict(toolchain.core_parse_transforms)
    if getattr(ctx.attrs, "use_global_parse_transforms", True):
        for parse_transform in toolchain.parse_transforms:
            if (
                # add parse_transform if there is no filter set
                not parse_transform in toolchain.parse_transforms_filters
                or
                # or if module is listed in the filter and add conditionally
                module_name(src) in toolchain.parse_transforms_filters[parse_transform]
            ):
                parse_transforms[parse_transform] = toolchain.parse_transforms[parse_transform]
    args.add(parse_transforms.values())

    source = cmd_args(src, format = '{source, "{}"}')
    path_type = "{path_type, relative}"
    preserved_opts = _preserved_opts(opts)

    compile_info = cmd_args(source, path_type, preserved_opts, delimiter = ", ")

    # add relevant compile_info manually
    args.add(cmd_args(compile_info, format = "+{compile_info, [{}]}"))

    return args

def is_preservable_opt(opt: str) -> bool:
    return opt in ["+beam_debug_info", "+debug_info", "+inline", "+line_coverage"]

def _preserved_opts(opts: list[str]) -> cmd_args:
    """Options that should be preserved in the beam file despite +determinstic"""
    preserved = [opt.lstrip("+") for opt in opts if is_preservable_opt(opt)]

    joined = cmd_args(preserved, delimiter = ", ")
    return cmd_args(joined, format = "{options, [{}]}")

def module_name(in_file: Artifact) -> str:
    """Returns the basename of the artifact without extension"""
    end = in_file.basename.rfind(".")
    return in_file.basename[:end]

def _is_hrl(in_file: Artifact) -> bool:
    """Returns True if the artifact is a hrl file"""
    return _is_ext(in_file, ".hrl")

def _is_erl(in_file: Artifact) -> bool:
    """Returns True if the artifact is an erl file"""
    return _is_ext(in_file, ".erl")

def _is_yrl(in_file: Artifact) -> bool:
    """Returns True if the artifact is a yrl file"""
    return _is_ext(in_file, ".yrl")

def _is_xrl(in_file: Artifact) -> bool:
    """Returns True if the artifact is a xrl file"""
    return _is_ext(in_file, ".xrl")

def _is_config(in_file: Artifact) -> bool:
    """Returns True if the artifact is a config file"""
    return _is_ext(in_file, ".config")

def _is_ext(in_file: Artifact, extension: str) -> bool:
    """Returns True if the artifact has an extension listed in extensions"""
    return in_file.basename.endswith(extension)

def _dep_mapping_name(src: Artifact) -> str:
    return paths.join(
        _DEP_FILES_DIR,
        "{}.mapping".format(src.short_path),
    )

def _run_with_env(ctx: AnalysisContext, toolchain: Toolchain, args: cmd_args, **kwargs):
    """run interfact that injects env"""

    # use os_env defined in target if present
    if getattr(ctx.attrs, "os_env", None) == None:
        env = toolchain.env
    else:
        env = ctx.attrs.os_env

    if "env" in kwargs:
        kwargs["env"].update(env)
    else:
        kwargs["env"] = env

    ctx.actions.run(args, **kwargs)

# export

erlang_build = struct(
    prepare_build_environment = _prepare_build_environment,
    own_or_dep = _own_or_dep,
    build_steps = struct(
        generated_source_artifacts = _generated_source_artifacts,
        generate_include_artifacts = _generate_include_artifacts,
        generate_beam_artifacts = _generate_beam_artifacts,
    ),
    utils = struct(
        BUILD_DIR = _BUILD_DIR,
        is_hrl = _is_hrl,
        is_erl = _is_erl,
        is_yrl = _is_yrl,
        is_xrl = _is_xrl,
        is_config = _is_config,
        module_name = module_name,
        run_with_env = _run_with_env,
    ),
)
