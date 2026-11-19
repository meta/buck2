#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import argparse
import json
import os
import pathlib
import re
import shlex
import shutil
import subprocess
import sys

from writable import make_dir_recursively_writable, make_path_user_writable

_RE_TMPDIR_ENV_VAR = "TMPDIR"
_FILE_WRITE_FAILURE_MARKER = "could not write"


def _expand_args(command):
    for arg in command:
        if arg.startswith("@"):
            with open(arg[1:]) as argsfile:
                return [line.rstrip() for line in argsfile.readlines()]

    return []


def _get_modulemap(args):
    for i in range(len(args)):
        if args[i].startswith("-explicit-swift-module-map-file"):
            if args[i + 1].startswith("-Xfrontend"):
                path = args[i + 2]
            else:
                path = args[i + 1]

            with open(path.strip()) as modulemap:
                return json.load(modulemap)

    return None


def _get_modulename(args):
    return args[args.index("-module-name") + 1]


def _get_modules(command):
    args = _expand_args(command)
    modules = set()
    modules.add(_get_modulename(args))
    modulemap = _get_modulemap(args)
    if modulemap is None:
        return modules

    for entry in modulemap:
        # We only remove prefixes from first party modules.
        if "sdk_deps" in entry.get("clangModulePath", ""):
            pass
        elif "sdk_deps" in entry.get("modulePath", ""):
            pass
        else:
            modules.add(entry["moduleName"])

    return modules


def _remove_swiftinterface_module_prefixes(command):
    interface_path = command[command.index("-emit-module-interface-path") + 1]
    modules = _get_modules(command)
    pattern = re.compile(r"(\w+)\.([\w.]+)")

    def replace_module_prefixes(match):
        if match.group(1) in modules:
            return match.group(2)
        else:
            return match.group(0)

    with open(interface_path, "r+") as f:
        interface = f.read()
        f.seek(0)
        f.write(pattern.sub(replace_module_prefixes, interface))
        f.truncate()


def _make_path_user_writable(path: str) -> None:
    # Ensure the path is writable by the user. This is required for Swift
    # Incremental Remote Actions, where this path may or may not be
    # pre-populated from a previous action. We need to check because we won't
    # know if we have these paths populated until runtime.
    if "INSIDE_RE_WORKER" not in os.environ:
        return

    if not os.path.exists(path):
        return

    backup_path = f"{path}.bak"
    shutil.move(path, backup_path)
    if os.path.isdir(backup_path):
        shutil.copytree(backup_path, path)
        make_dir_recursively_writable(path)
        # Remove the original dir. Unlinking its files needs write
        # permission on parent dir
        for dirpath, _dirnames, _filenames in os.walk(backup_path):
            make_path_user_writable(dirpath)
        shutil.rmtree(backup_path)
    else:
        shutil.copy2(backup_path, path)
        pathlib.Path(backup_path).unlink()
        make_path_user_writable(path)


def _rewrite_dependency_file(command, out_path):
    if not out_path:
        raise RuntimeError("-emit-dependencies requires -dependencies-file-output")

    # The compiler will output d files in Makefile format with abolute paths,
    # Buck expects line separated relative paths with no input prefix.
    output_file_map_path = command[command.index("-output-file-map") + 1]
    with open(output_file_map_path) as f:
        output_file_map = json.load(f)

    deps_file_path = output_file_map[""]["dependencies"]
    with open(deps_file_path, encoding="utf-8") as f:
        for line in f:
            # We have multiple entries for the supplementary outputs of the
            # compilation action. We have two cases we care about:
            #  1. swiftmodule output
            #  2. object file output
            # Other supplemenatary outputs like swiftsourceinfo are not
            # tracked.
            output, inputs = line.split(" : ")
            if output.endswith(".swiftmodule") or output.endswith(".o"):
                dependencies = shlex.split(inputs)
                break

    # Sanity check that we only track relative paths
    relative_paths = []
    for path in dependencies:
        if "/usr/lib/swift/host/" in path:
            # We don't track toolchain internal paths.
            continue
        elif path.startswith("/"):
            # This can occur for Xcode toolchain plugins like libSwiftUIMacros.dylib
            print(f"Dependency file contains absolute path: {path}", file=sys.stderr)
        else:
            relative_paths.append(os.path.normpath(path))

    _make_path_user_writable(out_path)
    with open(out_path, "w") as f:
        f.write("\n".join(sorted(relative_paths)))
        f.write("\n")


def _get_output_file_map(args):
    try:
        i = args.index("-output-file-map")
        filename = args[i + 1]
    except (ValueError, IndexError):
        raise RuntimeError("Failed to find -output-file-map in args")

    with open(filename) as f:
        return json.load(f)


def _get_serialized_diagnostics_frontend_flag(args):
    for a in args:
        if a.startswith("-serialize-diagnostics-path="):
            return a
    return None


def _get_serialized_diagnostics_path(args):
    # The diagnostics can be specified via frontend args or in the output file
    # map.
    frontend_arg = _get_serialized_diagnostics_frontend_flag(args)
    if frontend_arg:
        return frontend_arg.split("=")[1]

    output_file_map = _get_output_file_map(args)
    module_entries = output_file_map.get("", {})
    if "diagnostics" in module_entries:
        return module_entries["diagnostics"]
    else:
        raise RuntimeError("Failed to find diagnostics in output file map")


def _parse_wrapper_args(
    allargs: list[str],
) -> tuple[list[str], argparse.ArgumentParser]:
    driver_args = []
    wrapper_args = []
    i = 0
    while i < len(allargs):
        arg = allargs[i]
        if arg == "-Xwrapper":
            if i == len(allargs) - 1:
                raise RuntimeError("Missing argument to -Xwrapper")

            wrapper_args.append(allargs[i + 1])
            i += 2
        else:
            driver_args.append(arg)
            i += 1

    parser = argparse.ArgumentParser()
    parser.add_argument(
        "-dependencies-file-output",
        help="Path to write Buck format dependencies file to",
    )
    parser.add_argument(
        "-ignore-errors",
        action="store_true",
        help="Used to ignore errors during index store generation",
    )
    parser.add_argument(
        "-json-error-output-path",
        help="Path to write error output in JSON format",
    )
    parser.add_argument(
        "-remove-module-prefixes",
        action="store_true",
        help="Hack to remove module prefixes from swiftinterface output",
    )
    parser.add_argument(
        "-serialized-diagnostics-to-json",
        help="Path to serialized diagnostics to JSON transformer",
    )
    parser.add_argument(
        "-skip-incremental-outputs",
        action="store_true",
        help="Hack to skip writing incremental outputs to avoid cache invalidation",
    )
    parser.add_argument(
        "--writable-incremental-paths",
        nargs="*",
        help="Paths that should be made writable for incremental compilation",
    )
    parser.add_argument(
        "--no-file-prefix-map",
        action="store_true",
        help="Don't use -file-prefix-map or -coverage-prefix-map options",
    )
    parsed_args = parser.parse_args(wrapper_args)

    if (
        parsed_args.json_error_output_path
        and not parsed_args.serialized_diagnostics_to_json
    ):
        raise RuntimeError(
            "-json-error-output-path requires -serialized-diagnostics-to-json to transform the serialized diagnostics."
        )

    return driver_args, parsed_args


def main():
    env = os.environ.copy()
    if "INSIDE_RE_WORKER" in env and _RE_TMPDIR_ENV_VAR in env:
        # Use $TMPDIR for the module cache location. This
        # will be set to a unique location for each RE action
        # which will avoid sharing modules across RE actions.
        # This is necessary as the inputs to the modules will
        # be transient and can be removed at any point, causing
        # module validation errors to fail builds.
        # https://github.com/llvm/llvm-project/blob/main/clang/lib/Driver/ToolChains/Clang.cpp#L3709
        env["CLANG_MODULE_CACHE_PATH"] = os.path.join(
            env[_RE_TMPDIR_ENV_VAR], "buck-module-cache"
        )
    else:
        # For local actions use a shared module cache location.
        # This should be safe to share across the other local
        # compilation actions.
        env["CLANG_MODULE_CACHE_PATH"] = "/tmp/buck-module-cache"

    # Separate the driver args from the wrapper args
    command, wrapper_args = _parse_wrapper_args(sys.argv[1:])

    # Use relative paths for debug information and index information,
    # so we generate relocatable files.
    #
    # We need to use the path where the action is run (both locally and on RE),
    # which is not known when we define the action.
    if not wrapper_args.no_file_prefix_map:
        command += [
            # Macro expansions get materialized in the temporary directory, which
            # varies between local and remote actions. For local actions this will
            # be a subdir of the CWD, so this needs to be the first map entry.
            # We also need this prefix for the clang module cache path if we are
            # not using explicit modules with remote actions.
            "-file-prefix-map",
            f"{env.get(_RE_TMPDIR_ENV_VAR, '/tmp').rstrip('/')}=/tmp",
            "-file-prefix-map",
            f"{os.getcwd()}/=",
            "-file-prefix-map",
            f"{os.getcwd()}=.",
        ]

        # Apply a coverage prefix map for the current directory
        # to make file path metadata relocatable stripping
        # the current directory from it.
        #
        # This overrides -file-prefix-map.
        command += [
            "-coverage-prefix-map",
            f"{os.getcwd()}=.",
        ]

    if wrapper_args.skip_incremental_outputs:
        command = _process_skip_incremental_outputs(command)

    writable_args = ["-emit-objc-header-path", "-emit-module-path"]

    for arg in writable_args:
        if arg in command:
            idx = command.index(arg)
            file_path = command[idx + 1]
            _make_path_user_writable(file_path)

    if wrapper_args.writable_incremental_paths:
        for file_path in wrapper_args.writable_incremental_paths:
            _make_path_user_writable(file_path)
        output_file_map_path = command[command.index("-output-file-map") + 1]
        with open(output_file_map_path) as f:
            output_file_map = json.load(f)
            for value in output_file_map.values():
                for subkey in [
                    "swift-dependencies",
                    "dependencies",
                    "emit-module-dependencies",
                    "emit-module-diagnostics",
                    "diagnostics",
                    "object",
                ]:
                    maybe_path = value.get(subkey, None)
                    if maybe_path:
                        _make_path_user_writable(maybe_path)

    # Ensure the directories referenced by the output file map exist.
    # This can happen for Swift Incremental mode where there's no
    # previous incremental action available
    if "-output-file-map" in command:
        with open(command[command.index("-output-file-map") + 1]) as f:
            for entry in json.load(f).values():
                for maybe_path in entry.values():
                    if isinstance(maybe_path, str) and os.path.dirname(maybe_path):
                        os.makedirs(os.path.dirname(maybe_path), exist_ok=True)

    result = subprocess.run(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        encoding=sys.stdout.encoding,
        env=env,
    )

    print(result.stdout, file=sys.stdout, end="")
    print(result.stderr, file=sys.stderr, end="")

    if wrapper_args.json_error_output_path:
        _make_path_user_writable(wrapper_args.json_error_output_path)
        with open(wrapper_args.json_error_output_path, "w") as json_out:
            # Don't bother running the diagnostics deserializer if compilation
            # succeeded as the output will never be used.
            if result.returncode == 0 or wrapper_args.ignore_errors:
                json_out.write("[]")
            else:
                # Get the serialized diagnostics output from the output file map.
                serialized_diags = _get_serialized_diagnostics_path(command)

                # Convert the diagnostics to JSON for the Buck error handler.
                subprocess.run(
                    [wrapper_args.serialized_diagnostics_to_json, serialized_diags],
                    stdout=json_out,
                    check=True,
                )

    if result.returncode == 0:
        # The Swift compiler will return an exit code of 0 and warn when it
        # cannot write auxiliary files. Detect and error so that the action
        # is not cached.
        failed_write = (
            _FILE_WRITE_FAILURE_MARKER in result.stdout
            or _FILE_WRITE_FAILURE_MARKER in result.stderr
        )
        if failed_write:
            print(
                "Detected Swift compiler file write error but compiler exited with code 0, failing command..."
            )
            sys.exit(1)

        # Rewrite .d files for the format that Buck requires.
        if "-emit-dependencies" in command:
            _rewrite_dependency_file(command, wrapper_args.dependencies_file_output)

        # https://github.com/swiftlang/swift/issues/56573
        if wrapper_args.remove_module_prefixes:
            _remove_swiftinterface_module_prefixes(command)

    if wrapper_args.ignore_errors:
        sys.exit(0)
    else:
        sys.exit(result.returncode)


_SWIFT_FILES_ARGSFILE = "_swift_srcs"


def _process_skip_incremental_outputs(command):
    output_file_map = command[command.index("-output-file-map") + 1]
    output_dir = os.path.dirname(os.path.dirname(output_file_map))

    module_swiftdeps = (
        f"{output_dir}/__swift_incremental__/swiftdeps/module-build-record.priors"
    )
    output_file_map_content = {
        "": {
            "swift-dependencies": module_swiftdeps,
        },
    }

    swift_files = _get_swift_files_to_compile(command)
    for swift_file in swift_files:
        file_name = swift_file.split("/")[-1]
        output_artifact = f"{output_dir}/__swift_incremental__/objects/{file_name}.o"
        swiftdeps_artifact = (
            f"{output_dir}/__swift_incremental__/swiftdeps/{file_name}.swiftdeps"
        )
        output_file_map_content[swift_file] = {
            "object": output_artifact,
            "swift-dependencies": swiftdeps_artifact,
        }

    with open(output_file_map, "w") as f:
        json.dump(output_file_map_content, f, indent=2)

    return command


def _get_swift_files_to_compile(command):
    for arg in command:
        if arg.endswith(_SWIFT_FILES_ARGSFILE):
            path = arg[1:]  # Remove leading @
            with open(path) as swift_files:
                return [
                    line.rstrip().strip('"').strip("'")
                    for line in swift_files.readlines()
                ]

    raise Exception(f"No {_SWIFT_FILES_ARGSFILE} found!")


if __name__ == "__main__":
    main()
