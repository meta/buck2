# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:compile_types.bzl", "CudaDistributedCompileOutput", "CxxCompileCommand", "HeadersDepFiles")
load("@prelude//cxx:compiler.bzl", "get_output_flags")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//cxx:headers.bzl", "add_headers_dep_files")
load(
    "@prelude//utils:utils.bzl",
    "as_output",
)

CudaCompileInfo = record(
    # Output base filename without extension
    filename = field(str),
    # Buck action identifier
    identifier = field(str),
    # Output sub-directory where all CUDA compilation artifacts will go to
    output_prefix = field(str),
    uses_content_based_paths = field(bool),
)

CudaCompileStyle = enum(
    # Use NVCC as the compiler driver and compile a CUDA file in a single Buck
    # action.
    "mono",
    # NVCC provides the compilation plan, but use one Buck action per compilation
    # sub-command.
    "dist",
)

CudaDistributedCompileSpec = record(
    cuda_compile_info = field(CudaCompileInfo),
    original_cmd = field(cmd_args),
    argsfile_args = field(cmd_args),
    file_prefix_specs = field(Artifact | None),
    output_declared_artifact = field(OutputArtifact),
    src = field(Artifact),
)

PreparedCudaCommand = record(
    cmd_node = field(dict[str, typing.Any]),
    exe = field(str),
    parts = field(list[typing.Any]),
)

def declare_cuda_dist_compile_output(actions: AnalysisActions, cuda_compile_info: CudaCompileInfo) -> CudaDistributedCompileOutput:
    """
    Declare output artifacts for CUDA distributed compilation upfront.
    This should be called during analysis before the dynamic action.
    """
    content_based = cuda_compile_info.uses_content_based_paths

    # Create the following files for each CUDA file:
    # - Envvars to run the NVCC sub-commands with.
    # - A dependency graph of the NVCC sub-commands.
    # - Argsfile for the host compiler.
    env = actions.declare_output(
        cuda_compile_info.output_prefix,
        "{}.env".format(cuda_compile_info.filename),
        has_content_based_path = content_based,
    )
    subcmds = actions.declare_output(
        cuda_compile_info.output_prefix,
        "{}.json".format(cuda_compile_info.filename),
        has_content_based_path = content_based,
    )
    hostcc_argsfile = actions.declare_output(
        cuda_compile_info.output_prefix,
        "{}.hostcc_argsfile".format(cuda_compile_info.filename),
        has_content_based_path = content_based,
    )
    return CudaDistributedCompileOutput(
        nvcc_dag = subcmds,
        nvcc_env = env,
        hostcc_argsfile = hostcc_argsfile,
    )

def cuda_mono_compile(
    actions: AnalysisActions,
    cmd: cmd_args,
    object: OutputArtifact,
    src: Artifact,
    cxx_compile_cmd: CxxCompileCommand,
    cuda_compile_info: CudaCompileInfo,
    action_dep_files: dict[str, ArtifactTag],
    allow_dep_file_cache_upload: bool,
    error_handler: [typing.Callable, None],
) -> None:
    """
    Compile a CUDA file monolithically using NVCC as the compiler driver.
    All compilation happens in a single Buck action.
    """

    # Bind the object output for monolithic NVCC compilation.
    cmd.add(get_output_flags(cxx_compile_cmd.compiler_type, object))
    headers_dep_files = cxx_compile_cmd.headers_dep_files
    if headers_dep_files:
        cmd = add_headers_dep_files(
            actions,
            cmd,
            headers_dep_files,
            src,
            cuda_compile_info.filename,
            action_dep_files,
        )
    actions.run(
        cmd,
        category = cxx_compile_cmd.category,
        identifier = cuda_compile_info.identifier,
        dep_files = action_dep_files,
        allow_cache_upload = cxx_compile_cmd.allow_cache_upload,
        allow_dep_file_cache_upload = allow_dep_file_cache_upload,
        error_handler = error_handler,
    )

def cuda_distributed_compile(
    actions: AnalysisActions,
    cmd: cmd_args,
    object: OutputArtifact,
    cuda_dist_output: CudaDistributedCompileOutput,
    src: Artifact,
    cxx_compile_cmd: CxxCompileCommand,
    cuda_compile_info: CudaCompileInfo,
    cuda_prepare_cmd: cmd_args | None,
    shared_plan_identifier: str | None,
) -> CudaDistributedCompileSpec:
    """
    Set up distributed compilation for a CUDA file: optionally register the
    nvcc -dryrun prepare action that produces the target's shared compilation
    plan, and return the spec from which create_cuda_distributed_compiles
    later creates one Buck action per NVCC sub-command.

    Compilation modes that require a whole-program device link step are NOT
    supported here: relocatable device code (-rdc=true / --device-c), device
    LTO (-dlto), and PTX-only codegen all make nvcc -dryrun emit an nvlink
    sub-command that dist NVCC cannot distribute. Such targets must set
    cuda_compile_style = "mono". If one of these modes reaches this path, the
    dryrun parser (tools/build/cuda/nvcc_dryrun.py) fails the
    cuda_compile_prepare action with an actionable error that names the source
    .cu and points to the mono escape hatch.
    """
    hostcc_argsfile = cuda_dist_output.hostcc_argsfile

    # The object is produced by the replayed plan sub-actions, not by this
    # command, so embed its path as a string rather than binding the output.
    cmd_with_output = cmd_args(cmd, ["-o", object.short_path])
    original_cmd = cmd_args(
        cmd_with_output,
        hidden = [
            cxx_compile_cmd.base_compile_cmd,
            cxx_compile_cmd.argsfile.cmd_form,
        ],
    )

    # The dry run never opens this file. It matches the flag, echoed as one
    # `-specs=<path>` token, so the plan can carry a placeholder in place of a
    # path that moves with the header closure.
    file_prefix_specs = cxx_compile_cmd.argsfile.file_prefix_specs

    if cuda_prepare_cmd != None:
        prepare_cmd = cmd_args(
            cuda_prepare_cmd,
            ["-o", object.short_path],
            [
                "-_NVCC_DRYRUN_",
                "-_NVCC_HOSTCC_ARGSFILE_",
                as_output(hostcc_argsfile),
                "-_NVCC_DRYRUN_ENV_OUT_",
                as_output(cuda_dist_output.nvcc_env),
                "-_NVCC_DRYRUN_DAG_OUT_",
                as_output(cuda_dist_output.nvcc_dag),
            ],
        )
        if file_prefix_specs != None:
            prepare_cmd.add(
                "-_NVCC_FILE_PREFIX_SPECS_",
                cmd_args(file_prefix_specs, format = "-specs={}"),
            )

        # Run nvcc with -dryrun to create the inputs needed for dist nvcc.
        actions.run(prepare_cmd, category = "cuda_compile_prepare", identifier = shared_plan_identifier or cuda_compile_info.identifier)

    return CudaDistributedCompileSpec(
        cuda_compile_info = cuda_compile_info,
        original_cmd = original_cmd,
        argsfile_args = cxx_compile_cmd.argsfile.args,
        file_prefix_specs = file_prefix_specs,
        output_declared_artifact = object,
        src = src,
    )

# Keep the old cuda_compile function for backward compatibility
def cuda_compile(
    actions: AnalysisActions,
    cmd: cmd_args,
    object: OutputArtifact,
    src: Artifact,
    cxx_compile_cmd: CxxCompileCommand,
    cuda_compile_info: CudaCompileInfo,
    action_dep_files: dict[str, ArtifactTag],
    allow_dep_file_cache_upload: bool,
    error_handler: [typing.Callable, None],
    cuda_compile_style: CudaCompileStyle | None,
    cuda_dist_output: CudaDistributedCompileOutput | None = None,
    cuda_prepare_cmd: cmd_args | None = None,
    shared_plan_identifier: str | None = None,
) -> CudaDistributedCompileSpec | None:
    """
    Compile a CUDA file using either monolithic or distributed compilation.
    This is a convenience function that dispatches to the appropriate implementation.
    """
    if cuda_compile_style == CudaCompileStyle("mono"):
        cuda_mono_compile(
            actions,
            cmd,
            object,
            src,
            cxx_compile_cmd,
            cuda_compile_info,
            action_dep_files,
            allow_dep_file_cache_upload,
            error_handler,
        )
        return None
    elif cuda_compile_style == CudaCompileStyle("dist"):
        if cuda_dist_output == None:
            fail("cuda_dist_output is required for distributed CUDA compilation")
        return cuda_distributed_compile(
            actions,
            cmd,
            object,
            cuda_dist_output,
            src,
            cxx_compile_cmd,
            cuda_compile_info,
            cuda_prepare_cmd,
            shared_plan_identifier,
        )
    else:
        fail("Unsupported CUDA compile style: {}".format(cuda_compile_style))

def create_cuda_distributed_compiles(
    actions: AnalysisActions,
    toolchain: CxxToolchainInfo,
    cuda_dist_output: CudaDistributedCompileOutput,
    specs: list[CudaDistributedCompileSpec],
    allow_cache_upload: bool,
    cuda_cxx_compile_inputs: cmd_args,
    headers_dep_files: HeadersDepFiles | None,
) -> None:
    if not specs:
        return

    # The argsfile is shared by the target's sources, so one fingerprint covers
    # them all. It carries the same flags with content-based paths rendered as a
    # placeholder: it moves when a flag really changes, not when a path does.
    hostcc_argsfile_fingerprint = None
    if headers_dep_files != None:
        hostcc_argsfile_fingerprint, _ = actions.write(
            "__redacted__/{}.hostcc_argsfile_fingerprint".format(specs[0].cuda_compile_info.filename),
            cmd_args(specs[0].original_cmd, specs[0].argsfile_args, quote = "shell"),
            allow_args = True,
            has_content_based_path = specs[0].cuda_compile_info.uses_content_based_paths,
            use_dep_files_placeholder_for_content_based_paths = True,
        )

    actions.dynamic_output_new(
        _nvcc_dynamic_compile_rule(
            toolchain = toolchain,
            cuda_compile_infos = [spec.cuda_compile_info for spec in specs],
            srcs = [spec.src for spec in specs],
            allow_cache_upload = allow_cache_upload,
            cuda_cxx_compile_inputs = cuda_cxx_compile_inputs,
            headers_dep_files = headers_dep_files,
            original_cmds = [spec.original_cmd for spec in specs],
            hostcc_argsfile = cuda_dist_output.hostcc_argsfile,
            hostcc_argsfile_fingerprint = hostcc_argsfile_fingerprint,
            file_prefix_specs = specs[0].file_prefix_specs,
            plan_artifact = cuda_dist_output.nvcc_dag,
            env_artifact = cuda_dist_output.nvcc_env,
            output_declared_artifacts = [spec.output_declared_artifact for spec in specs],
        )
    )

def _create_file_to_artifact_map(
    actions: AnalysisActions,
    plan_json: list[dict[str, typing.Any]],
    src: Artifact,
    cuda_compile_info: CudaCompileInfo,
    output_declared_artifact: OutputArtifact,
    uses_content_based_paths: bool,
) -> dict[str, Artifact | OutputArtifact]:
    # Create artifacts for all intermediate input and output files.
    file2artifact = {}
    for cmd_node in plan_json:
        node_inputs = cmd_node["inputs"]
        node_outputs = cmd_node["outputs"]
        for input in node_inputs:
            if input not in file2artifact:
                if input.endswith(".cu"):
                    file2artifact[input] = src
                else:
                    input_artifact = actions.declare_output(
                        "__cuda_intermediates__",
                        "{}/{}".format(cuda_compile_info.filename, input),
                        has_content_based_path = uses_content_based_paths,
                    )
                    file2artifact[input] = input_artifact
        for output in node_outputs:
            if output not in file2artifact:
                if output.endswith(".o"):
                    file2artifact[output] = output_declared_artifact
                else:
                    output_artifact = actions.declare_output(
                        "__cuda_intermediates__",
                        "{}/{}".format(cuda_compile_info.filename, output),
                        has_content_based_path = uses_content_based_paths,
                    )
                    file2artifact[output] = output_artifact
    return file2artifact

def _create_nvcc_subcmd_env(env_artifact: ArtifactValue) -> dict[str, str]:
    # Create the nvcc envvars for the sub-commands.
    subcmd_env = {}
    for line in env_artifact.read_string().splitlines():
        key, value = line.split("=", 1)
        subcmd_env[key] = value
    return subcmd_env

def _include_symlinked_stubs_dir(
    actions: AnalysisActions,
    cuda_compile_info: CudaCompileInfo,
    file2artifact: dict[str, typing.Any],
    subcmd: cmd_args,
) -> None:
    """
    .cudafe1.stub.c and .fatbin.c files are hardcoded into the cudafe1.cpp file
    and its includes like below:

    #include "add.cu.o.compute_90a.cudafe1.stub.c"

    Before content-based hashing, this worked fine because all outputs were under
    the same directory. However, with content-based hashing this no longer works
    because the output files are put under the output_artifacts sub-directory.

    Fix this by creating a directory containing symlinks to the actual stubs, and
    adding it to the cuda_cxx_compile include search path.
    """
    stubs_dir = actions.declare_output(
        "__stubs__",
        cuda_compile_info.filename,
        dir = True,
        has_content_based_path = True,
    )
    stubs = {}
    for file, artifact in file2artifact.items():
        if file.endswith(".cudafe1.stub.c") or file.endswith(".fatbin.c"):
            # Remove the parent paths because the includes are the filenames only.
            # Each stubs directory is scoped to one CUDA source file.
            stubs[artifact.basename] = artifact
    symlinked_dir = actions.symlinked_dir(
        stubs_dir,
        stubs,
        has_content_based_path = True,
    )
    subcmd.add(cmd_args(symlinked_dir, format = "-I{}"))

def _prepare_cuda_command(cmd_node: dict[str, typing.Any], hostcc_wp_form: cmd_args, specs_form: cmd_args | None) -> PreparedCudaCommand:
    parts = []
    common = cmd_args()
    for token in cmd_node["cmd"][1:]:
        placeholder = None
        if "{input}" in token:
            placeholder = "input"
        elif "{output}" in token:
            placeholder = "output"
        elif "{source_path}" in token:
            placeholder = "source_path"

        if placeholder != None:
            parts.append(("common", common))
            left, right = token.split("{" + placeholder + "}", 1)

            # Replay substitutes exactly one placeholder per token; a second
            # one would be passed through literally.
            for kind in ("input", "output", "source_path"):
                if "{" + kind + "}" in left + right:
                    fail("plan token carries multiple placeholders: {}".format(token))
            parts.append((placeholder, (left, right)))
            common = cmd_args()
        elif token.startswith("-Wp,@"):
            common.add(hostcc_wp_form)
        elif token == "{file_prefix_specs}":
            if specs_form == None:
                fail("CUDA plan references file-prefix specs without a specs artifact")
            common.add(specs_form)
        else:
            common.add(token)
    parts.append(("common", common))
    return PreparedCudaCommand(cmd_node = cmd_node, exe = cmd_node["cmd"][0], parts = parts)

def _cuda_replay_input_kind(category: str) -> str:
    # Device phase categories appear both arch-suffixed (`cuda_opt_sm90a`) and
    # bare: the plan parser emits the bare form whenever an intermediate file
    # name carries no compute_{arch} component, e.g. for arch suffixes its
    # normalization does not recognize.
    if category == "cuda_host_preproc" or category == "cuda_device_preproc" or category.startswith("cuda_device_preproc_"):
        return "preprocess"
    if category == "cuda_cxx_compile":
        return "host_compile"
    if category in ("cudafepp", "cuda_fatbin", "cuda_opt", "cuda_cubin_lower") or category.startswith("cuda_opt_") or category.startswith("cuda_cubin_lower_"):
        return "cuda_tool"
    fail("Unsupported distributed CUDA phase category `{}`; update its input policy".format(category))

def _nvcc_dynamic_compile(
    actions: AnalysisActions,
    toolchain: CxxToolchainInfo,
    cuda_compile_infos: list[CudaCompileInfo],
    srcs: list[Artifact],
    allow_cache_upload: bool,
    cuda_cxx_compile_inputs: cmd_args,
    headers_dep_files: HeadersDepFiles | None,
    original_cmds: list[cmd_args],
    hostcc_argsfile: Artifact,
    hostcc_argsfile_fingerprint: Artifact | None,
    file_prefix_specs: Artifact | None,
    plan_artifact: ArtifactValue,
    env_artifact: ArtifactValue,
    output_declared_artifacts: list[OutputArtifact],
) -> list[Provider]:
    num_sources = len(cuda_compile_infos)
    if len(srcs) != num_sources or len(original_cmds) != num_sources or len(output_declared_artifacts) != num_sources:
        fail(
            "per-source dist CUDA lists must be the same length, got {}/{}/{}/{}".format(
                len(cuda_compile_infos),
                len(srcs),
                len(original_cmds),
                len(output_declared_artifacts),
            )
        )

    plan = plan_artifact.read_json()
    subcmd_env = _create_nvcc_subcmd_env(env_artifact)

    # The plan is shared across the target's sources, so these references are
    # built once from the representative source. Tagging lets dep-file filtering
    # drop their paths from each sub-action key; the untagged fingerprint is what
    # still reruns the sub-actions when a flag changes.
    if headers_dep_files != None and hostcc_argsfile_fingerprint != None:
        hostcc_wp_form = cmd_args(
            headers_dep_files.tag.tag_artifacts(hostcc_argsfile),
            format = "-Wp,@{}",
            hidden = [hostcc_argsfile_fingerprint],
        )
    else:
        hostcc_wp_form = cmd_args(hostcc_argsfile, format = "-Wp,@{}")

    specs_form = None
    if file_prefix_specs != None:
        if headers_dep_files != None:
            specs_form = cmd_args(
                headers_dep_files.tag.tag_artifacts(file_prefix_specs),
                format = "-specs={}",
            )
        else:
            specs_form = cmd_args(file_prefix_specs, format = "-specs={}")

    prepared_commands = [_prepare_cuda_command(cmd_node, hostcc_wp_form, specs_form) for cmd_node in plan]
    cuda_toolchain = getattr(toolchain.cuda_compiler_info, "compiler_for_dryrun", None) or toolchain.cuda_compiler_info.compiler
    cuda_tool_inputs = cmd_args(hidden = [cuda_toolchain])

    for cuda_compile_info, src, original_cmd, output_declared_artifact in zip(
        cuda_compile_infos,
        srcs,
        original_cmds,
        output_declared_artifacts,
    ):
        content_based = cuda_compile_info.uses_content_based_paths
        file2artifact = _create_file_to_artifact_map(
            actions,
            plan,
            src,
            cuda_compile_info,
            output_declared_artifact,
            content_based,
        )

        category_counts = {}
        for prepared in prepared_commands:
            cmd_node = prepared.cmd_node
            subcmd = cmd_args()
            exe = prepared.exe
            input_kind = _cuda_replay_input_kind(cmd_node["category"])
            if input_kind == "preprocess":
                subcmd.add(cmd_args(hidden = original_cmd))
            elif input_kind == "host_compile":
                subcmd.add(cmd_args(hidden = cuda_cxx_compile_inputs))
            else:
                subcmd.add(cuda_tool_inputs)

            if "ptxas" in exe:
                # Ptxas occasionally produces an empty output. The root cause
                # is unknown as we're unable to reproduce it locally. Check the
                # output is not empty
                subcmd.add(toolchain.internal_tools.check_nonempty_output)
            subcmd.add(exe)

            # The stubs directory must lead the include path so its generated
            # stub files shadow any same-named file in later include dirs.
            if content_based and cmd_node["category"] == "cuda_cxx_compile":
                _include_symlinked_stubs_dir(actions, cuda_compile_info, file2artifact, subcmd)

            input_index = 0
            output_index = 0
            for kind, value in prepared.parts:
                if kind == "common":
                    subcmd.add(value)
                elif kind == "input":
                    input = cmd_node["inputs"][input_index]
                    input_index += 1
                    subcmd.add(cmd_args([value[0], file2artifact[input], value[1]], delimiter = ""))
                elif kind == "output":
                    output = cmd_node["outputs"][output_index]
                    output_index += 1
                    artifact = file2artifact[output]
                    bindable = artifact.as_output() if isinstance(artifact, Artifact) else artifact
                    subcmd.add(cmd_args([value[0], bindable, value[1]], delimiter = ""))
                elif kind == "source_path":
                    # The source path is a diagnostic stamp (fatbinary --ident,
                    # DWARF attribution): the sub-action records the path but
                    # never reads the file.
                    subcmd.add(cmd_args([value[0], src, value[1]], delimiter = "", ignore_artifacts = True))
                else:
                    fail("unhandled placeholder kind: {}".format(kind))

            # Some nodes have hidden dependencies (deps that don't appear in
            # the cmd). Add them to the hidden field of cmd_args.
            if cmd_node["hidden"]:
                subcmd.add(cmd_args(hidden = [file2artifact[f] for f in cmd_node["hidden"]]))

            # Host-compiler sub-actions pin a declared header closure; dep files
            # let the compiler report what it actually read so buck can prune the
            # rest from the action key.
            action_dep_files = {}
            if input_kind != "cuda_tool" and headers_dep_files != None:
                # Categories can repeat within a plan, so disambiguate with a
                # per-category ordinal.
                ordinal = category_counts.get(cmd_node["category"], 0)
                category_counts[cmd_node["category"]] = ordinal + 1
                subcmd = add_headers_dep_files(
                    actions,
                    subcmd,
                    headers_dep_files,
                    src,
                    "{}/{}/{}".format(cuda_compile_info.filename, cmd_node["category"], ordinal),
                    action_dep_files,
                )

            actions.run(
                subcmd,
                category = cmd_node["category"],
                env = subcmd_env,
                identifier = cuda_compile_info.identifier,
                dep_files = action_dep_files,
                allow_cache_upload = allow_cache_upload,
                allow_dep_file_cache_upload = toolchain.allow_dep_file_cache_upload and bool(action_dep_files),
                prefer_remote = True if "preproc" in cmd_node["category"] else False,
            )

    return [DefaultInfo()]

_nvcc_dynamic_compile_rule = dynamic_actions(
    impl = _nvcc_dynamic_compile,
    attrs = {
        "allow_cache_upload": dynattrs.value(bool),
        "cuda_compile_infos": dynattrs.list(dynattrs.value(CudaCompileInfo)),
        "cuda_cxx_compile_inputs": dynattrs.value(cmd_args),
        "env_artifact": dynattrs.artifact_value(),
        "file_prefix_specs": dynattrs.option(dynattrs.value(Artifact)),
        "headers_dep_files": dynattrs.option(dynattrs.value(HeadersDepFiles)),
        "hostcc_argsfile": dynattrs.value(Artifact),
        "hostcc_argsfile_fingerprint": dynattrs.option(dynattrs.value(Artifact)),
        "original_cmds": dynattrs.list(dynattrs.value(cmd_args)),
        "output_declared_artifacts": dynattrs.list(dynattrs.output()),
        "plan_artifact": dynattrs.artifact_value(),
        "srcs": dynattrs.list(dynattrs.value(Artifact)),
        "toolchain": dynattrs.value(CxxToolchainInfo),
    },
)
