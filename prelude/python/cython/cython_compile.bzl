# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load(
    ":cython_providers.bzl",
    "CythonCompileOutput",
)
load(
    ":cython_toolchain.bzl",
    "CythonToolchainInfo",
)

def generate_cython_flags(
    generate_cpp: bool,
    cython_binding: bool = False,
    code_comments: bool = False,
    legacy_noexcept: bool = True,
    annotate: [str, None] = None,
    flags: list[str] = [],
) -> list[str]:
    """
    Build the list of flags to pass to the Cython compiler.

    Mirrors the flag construction logic from the existing cython_library macro's
    generate_cython_flags() function.
    """
    result = []

    # Language version (always Python 3)
    result.append("-3")

    # Fast fail on first error
    result.append("--fast-fail")

    # Annotation
    if annotate == "basic":
        result.append("--annotate")
    elif annotate == "fullc":
        result.append("--annotate-fullc")

    # C++ mode
    if generate_cpp:
        result.append("--cplus")

    # Cython binding mode
    result.extend(["-X", "binding={}".format(cython_binding)])

    # Code comments
    result.extend(["-X", "emit_code_comments={}".format(code_comments)])

    # Language level workaround (Cython v3.0 compat)
    result.extend(["-X", "language_level=3"])

    # Legacy implicit noexcept
    result.extend(["-X", "legacy_implicit_noexcept={}".format(legacy_noexcept)])

    # User-provided flags last so they can override defaults
    result.extend(flags)

    return result

def _build_package_init_tree(actions: AnalysisActions, package_path: str, identifier: str) -> dict[str, Artifact]:
    """
    Create __init__.py markers for all intermediate directories in the package path.
    """
    init_files = {}
    if not package_path:
        return init_files

    init_py = actions.write("__cython_init__/" + identifier + "/__init__.py", "", has_content_based_path = False)
    parts = package_path.split("/")
    for i in range(len(parts)):
        prefix = "/".join(parts[: i + 1])
        init_files[prefix + "/__init__.py"] = init_py

    return init_files

def cython_compile(
    actions: AnalysisActions,
    cython_toolchain: CythonToolchainInfo,
    src: Artifact,
    package_path: str,
    include_tree: [Artifact, None],
    flags: list[str],
    generate_cpp: bool,
    identifier: str,
    exec_os_type = None,
) -> CythonCompileOutput:
    """
    Run the Cython compiler on a single .pyx file.

    Args:
        actions: The AnalysisActions context.
        cython_toolchain: The CythonToolchainInfo provider.
        src: The .pyx source file artifact.
        package_path: The package path (e.g., "thrift/python").
        include_tree: Symlinked directory of transitive .pxd headers (or None).
        flags: Compiler flags to pass.
        generate_cpp: Whether to generate C++ (.cpp) or C (.c) output.
        identifier: A unique identifier for this compile action.
        exec_os_type: OsLookup provider for the execution platform.

    Returns:
        A CythonCompileOutput record with the generated source and header artifacts.
    """
    ext = ".cpp" if generate_cpp else ".c"
    module_name = paths.split_extension(paths.basename(src.short_path))[0]

    # Create a source layout directory with the .pyx under the package path
    # and __init__.py markers for package discovery
    src_layout = {}
    if package_path:
        src_layout[package_path + "/" + src.basename] = src
    else:
        src_layout[src.basename] = src

    # Add __init__.py markers for all intermediate package directories
    init_files = _build_package_init_tree(actions, package_path, identifier)
    src_layout.update(init_files)

    src_dir = actions.symlinked_dir(
        "__cython_src__/" + identifier,
        src_layout,
        has_content_based_path = False,
    )

    out_dir = actions.declare_output("__cython__/" + identifier, dir = True, has_content_based_path = False)

    cython_cmd = cmd_args()
    cython_cmd.add(cython_toolchain.compiler)
    cython_cmd.add(flags)

    if include_tree:
        cython_cmd.add("-I")
        cython_cmd.add(include_tree)

    # Output file path
    out_file = module_name + ext
    cython_cmd.add("-o")
    cython_cmd.add(cmd_args(out_dir.as_output(), format = "{}" + "/" + out_file))

    # Input file path
    if package_path:
        cython_cmd.add(cmd_args(src_dir, format = "{}" + "/" + package_path + "/" + src.basename))
    else:
        cython_cmd.add(cmd_args(src_dir, format = "{}" + "/" + src.basename))

    # Wrap with OS-appropriate mkdir to ensure the output directory exists
    # before the Cython compiler tries to write to it.
    is_windows = exec_os_type != None and exec_os_type[OsLookup].os == Os("windows")
    if is_windows:
        cmd = cmd_args([
            cmd_args(["cmd.exe", "/c", cmd_args([out_dir.as_output()], format = "if not exist {} md {}")]),
            "&&",
            cython_cmd,
        ])
    else:
        cmd = cmd_args(
            "/bin/sh",
            "-c",
            cmd_args(out_dir.as_output(), format = 'mkdir -p {} && "$@"'),
            '""',
            cython_cmd,
        )

    actions.run(
        cmd,
        category = "cython_compile",
        identifier = identifier,
        env = {"PYTHONHASHSEED": "0"},
    )

    return CythonCompileOutput(
        cpp_src = out_dir.project(out_file),
        api_header = out_dir.project(module_name + "_api.h"),
        public_header = out_dir.project(module_name + ".h"),
    )
