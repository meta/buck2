# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import argparse
import pathlib
import re
import shlex
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
from typing import List

import utils

_JAVA_FILE_EXTENSION = [".java"]


def _parse_args():
    parser = argparse.ArgumentParser(
        description="Tool to compile and create a jar for java library."
    )

    parser.add_argument(
        "--jar_builder_tool",
        type=str,
        required=True,
        help="a path to a jar builder tool",
    )
    parser.add_argument(
        "--output", type=pathlib.Path, required=True, help="a path to an output result"
    )
    parser.add_argument(
        "--skip_javac_run",
        action="store_true",
        help="flag that if set then need to skip javac execution",
    )
    parser.add_argument(
        "--javac_tool",
        type=pathlib.Path,
        help="a path to a java compiler tool",
    )
    parser.add_argument(
        "--javac_args_file",
        type=pathlib.Path,
        required=False,
        metavar="javac_args_file",
        help="path to file with stored args that need to be passed to java compiler",
    )
    parser.add_argument(
        "--multi_release_args_file",
        action="append",
        type=pathlib.Path,
        required=False,
        metavar="multi_release_args_file",
        help="path to file with stored args that need to be passed to java compiler for multi-release build",
    )
    parser.add_argument(
        "--zipped_sources_file",
        type=pathlib.Path,
        required=False,
        metavar="zipped_sources_file",
        help="path to file with stored zipped source files that need to be passed to java compiler",
    )

    parser.add_argument(
        "--javac_classpath_file",
        type=pathlib.Path,
        required=False,
        metavar="javac_classpath_file",
        help="path to file with stored classpath for java compilation",
    )
    parser.add_argument(
        "--javac_processors_classpath_file",
        type=pathlib.Path,
        required=False,
        metavar="javac_processors_classpath_file",
        help="path to file with stored classpath for java compilation processors",
    )
    parser.add_argument(
        "--javac_bootclasspath_file",
        type=pathlib.Path,
        required=False,
        metavar="javac_bootclasspath_file",
        help="path to file with stored bootclasspath for java compilation",
    )
    parser.add_argument(
        "--javac_jvm_args_file",
        type=pathlib.Path,
        required=False,
        metavar="javac_jvm_args_file",
        help="path to file with JVM args (-J prefixed) to pass directly to javac on the command line (not via @argfile)",
    )
    parser.add_argument(
        "--resources_dir",
        type=pathlib.Path,
        required=False,
        metavar="resources_dir",
        help="path to a directory with resources",
    )
    parser.add_argument(
        "--generated_sources_dir",
        type=pathlib.Path,
        required=False,
        metavar="generated_sources_dir",
        help="path to a directory where generated sources should be written",
    )
    parser.add_argument(
        "--manifest",
        type=pathlib.Path,
        required=False,
        metavar="manifest",
        help="a path to a custom manifest file",
    )
    parser.add_argument(
        "--remove_classes",
        type=pathlib.Path,
        help="paths to file with stored remove classes patterns",
    )
    parser.add_argument(
        "--additional_compiled_srcs",
        type=pathlib.Path,
        required=False,
        metavar="additional_compiled_srcs",
        help=".class files that should be packaged into the final jar",
    )
    parser.add_argument(
        "--concat_resources",
        action="store_true",
        help="if the jar tool should use parallel compression anc doncat intermediary jars",
    )

    return parser.parse_args()


def _get_multi_release_version(args_file: pathlib.Path) -> str:
    """
    Extracts the Java version from a filename ending with '.java<version>'.
    """
    version_match = re.search(r".java(\d+)$", str(args_file))
    return version_match.group(1) if version_match else None


def _get_multi_release_classpath(
    temp_build_dir: pathlib.Path,
    javac_classpath: pathlib.Path,
    javac_output: pathlib.Path,
) -> pathlib.Path:
    """
    Creates a copy of javac classpath at temp_build_dir/multi_release_classpath containing
    javac_classpath and javac_output, joined by the platform path separator.
    Returns the path to the created classpath file.
    """
    multi_release_classpath = temp_build_dir / "multi_release_classpath"
    if javac_classpath:
        shutil.copyfile(javac_classpath, multi_release_classpath)
        with multi_release_classpath.open("a", encoding="utf-8") as f:
            f.write(f":{javac_output}")
    else:
        multi_release_classpath.write_text(f"{javac_output}")
    return multi_release_classpath


def _get_multi_release_manifest(temp_build_dir: pathlib.Path) -> pathlib.Path:
    """
    Creates a Multi-Release manifest file at temp_build_dir/MANIFEST.MF
    Returns the path to the created manifest file.
    """
    multi_release_manifest = temp_build_dir / "MANIFEST.MF"
    multi_release_manifest.write_text("Manifest-Version: 1.0\nMulti-Release: true\n")
    return multi_release_manifest


def _run_javac(
    javac_tool: pathlib.Path,
    javac_args_file: pathlib.Path,
    javac_output: pathlib.Path,
    zipped_sources_file: pathlib.Path,
    javac_classpath_file: pathlib.Path,
    javac_processor_classpath_file: pathlib.Path,
    javac_bootclasspath_file: pathlib.Path,
    generated_sources_dir: pathlib.Path,
    temp_build_dir: pathlib.Path,
    javac_jvm_args_file: pathlib.Path = None,
) -> pathlib.Path:
    # make sure output folder exists
    javac_output.mkdir(parents=True, exist_ok=True)

    javac_cmd = [javac_tool]

    # JVM args must be passed directly on the command line (not via @argfile)
    # because javac does not support -J flags inside @argfiles.
    if javac_jvm_args_file:
        with open(javac_jvm_args_file) as f:
            for line in f:
                arg = line.strip()
                if arg:
                    javac_cmd.append(arg)

    args_file = javac_args_file
    if zipped_sources_file:
        args_file = utils.extract_source_files(
            zipped_sources_file, javac_args_file, _JAVA_FILE_EXTENSION, temp_build_dir
        )

    if utils.sources_are_present(args_file, _JAVA_FILE_EXTENSION):
        javac_cmd += ["@{}".format(args_file)]

        if javac_classpath_file:
            javac_cmd += ["-classpath", "@{}".format(javac_classpath_file)]

        if javac_bootclasspath_file:
            javac_cmd += ["-bootclasspath", "@{}".format(javac_bootclasspath_file)]

        if javac_processor_classpath_file:
            javac_cmd += [
                "-processorpath",
                "@{}".format(javac_processor_classpath_file),
            ]

        if generated_sources_dir:
            javac_cmd += [
                "-s",
                generated_sources_dir,
            ]

        javac_cmd += ["-g"]

        javac_cmd += ["-d", javac_output]
        utils.log_message(
            "executing command = '{}'".format(
                " ".join([shlex.quote(str(s)) for s in javac_cmd])
            )
        )

        p = subprocess.run(javac_cmd, stderr=subprocess.PIPE, text=True)
        if p.returncode != 0:
            print(
                f"javac command failed with exit code {p.returncode}. javac_tool={javac_tool}",
                file=sys.stderr,
            )
            if p.stderr:
                print(utils.pretty_exception_j(p.stderr), file=sys.stderr)
            sys.exit(p.returncode)

        return javac_output

    return javac_output


def _run_jar(
    jar_builder_tool: str,
    output_path: pathlib.Path,
    manifest_files: List[pathlib.Path],
    javac_output: pathlib.Path,
    resources_dir: pathlib.Path,
    additional_compiled_srcs: pathlib.Path,
    remove_classes_file: pathlib.Path,
    temp_build_dir: pathlib.Path,
    concat_resources: bool = False,
):
    jar_cmd = []
    jar_cmd.extend(utils.shlex_split(jar_builder_tool))

    content_to_pack_dirs = []
    if javac_output:
        content_to_pack_dirs.append(javac_output)
    if resources_dir:
        content_to_pack_dirs.append(resources_dir)
    if additional_compiled_srcs:
        content_to_pack_dirs.append(additional_compiled_srcs)

    entries_to_jar_file = temp_build_dir / "entries_to_jar.txt"
    entries_to_jar_file.write_text(
        "\n".join(str(path) for path in content_to_pack_dirs)
    )

    jar_cmd.extend(["--entries-to-jar", entries_to_jar_file])

    if manifest_files:
        jar_cmd.append("--merge-manifests")
        for manifest_file in manifest_files:
            jar_cmd.extend(["--manifest-file", manifest_file])
    if concat_resources:
        jar_cmd.extend(["--concat-jars"])

    if remove_classes_file:
        jar_cmd.extend(["--blocklist-patterns", remove_classes_file])
        jar_cmd.extend(
            ["--blocklist-patterns-matcher", "remove_classes_patterns_matcher"]
        )

    jar_cmd.extend(["--output", output_path])

    utils.log_message("jar_cmd: {}".format(" ".join([str(s) for s in jar_cmd])))
    utils.execute_command(jar_cmd)


def main():
    args = _parse_args()

    skip_javac_run = args.skip_javac_run
    javac_tool = args.javac_tool
    jar_builder_tool = args.jar_builder_tool
    output_path = args.output
    javac_args = args.javac_args_file
    multi_release_args_file = args.multi_release_args_file
    zipped_sources_file = args.zipped_sources_file
    javac_classpath = args.javac_classpath_file
    javac_processor_classpath = args.javac_processors_classpath_file
    javac_bootclasspath_file = args.javac_bootclasspath_file
    javac_jvm_args_file = args.javac_jvm_args_file
    resources_dir = args.resources_dir
    generated_sources_dir = args.generated_sources_dir
    manifest = args.manifest
    remove_classes_file = args.remove_classes
    additional_compiled_srcs = args.additional_compiled_srcs
    concat_resources = args.concat_resources
    manifest_files = [manifest] if manifest else []

    utils.log_message("javac_tool: {}".format(javac_tool))
    utils.log_message("jar_builder_tool: {}".format(jar_builder_tool))
    utils.log_message("output: {}".format(output_path))
    if skip_javac_run:
        utils.log_message("skip_javac_run: {}".format(skip_javac_run))
    if javac_args:
        utils.log_message("javac_args: {}".format(javac_args))
    if multi_release_args_file:
        for multi_release_args in multi_release_args_file:
            utils.log_message("multi_release_args: {}".format(multi_release_args))
    if zipped_sources_file:
        utils.log_message("zipped_sources_file: {}".format(zipped_sources_file))
    if javac_classpath:
        utils.log_message("javac_classpath: {}".format(javac_classpath))
    if javac_processor_classpath:
        utils.log_message(
            "javac_processor_classpath: {}".format(javac_processor_classpath)
        )
    if javac_bootclasspath_file:
        utils.log_message(
            "javac_bootclasspath_file: {}".format(javac_bootclasspath_file)
        )
    if resources_dir:
        utils.log_message("resources_dir: {}".format(resources_dir))
    if concat_resources:
        utils.log_message("concat_resources: {}".format(concat_resources))
    if generated_sources_dir:
        utils.log_message("generated_sources_dir: {}".format(generated_sources_dir))
        if not generated_sources_dir.exists():
            generated_sources_dir.mkdir()
    if manifest:
        utils.log_message("manifest: {}".format(manifest))
    if remove_classes_file:
        utils.log_message("remove classes file: {}".format(remove_classes_file))
    if additional_compiled_srcs:
        utils.log_message(
            "additional_compiled_srcs: {}".format(additional_compiled_srcs)
        )

    with TemporaryDirectory() as temp_dir:
        temp_build_dir = pathlib.Path(temp_dir)
        javac_output = temp_build_dir / "classes"
        javac_output.mkdir(parents=True, exist_ok=True)

        if not skip_javac_run:
            _run_javac(
                javac_tool,
                javac_args,
                javac_output,
                zipped_sources_file,
                javac_classpath,
                javac_processor_classpath,
                javac_bootclasspath_file,
                generated_sources_dir,
                temp_build_dir,
                javac_jvm_args_file,
            )
            if multi_release_args_file:
                # add javac_output as classpath for the multi-release build
                multi_release_classpath = _get_multi_release_classpath(
                    temp_build_dir, javac_classpath, javac_output
                )
                multi_release_manifest = _get_multi_release_manifest(temp_build_dir)
                manifest_files.append(multi_release_manifest)

                utils.log_message(f"multi_release_manifest: {multi_release_manifest}")
                utils.log_message(f"multi_release_classpath: {multi_release_classpath}")

                for multi_release_args in multi_release_args_file:
                    # The release file should be named as args.java<release_version>
                    multi_release_version = _get_multi_release_version(
                        multi_release_args
                    )
                    if not multi_release_version:
                        raise Exception(
                            f"Invalid release args file: {multi_release_args}"
                        )

                    # Multi-release classes are located in META-INF/versions/<multi_release_version>
                    multi_release_output = (
                        javac_output / f"META-INF/versions/{multi_release_version}"
                    )

                    utils.log_message(
                        f"javac multi_release {multi_release_version} at {multi_release_output}"
                    )

                    _run_javac(
                        javac_tool,
                        multi_release_args,
                        multi_release_output,
                        None,
                        multi_release_classpath,
                        javac_processor_classpath,
                        javac_bootclasspath_file,
                        generated_sources_dir,
                        temp_build_dir,
                    )

        _run_jar(
            jar_builder_tool,
            output_path,
            manifest_files,
            javac_output,
            resources_dir,
            additional_compiled_srcs,
            remove_classes_file,
            temp_build_dir,
            concat_resources,
        )


if __name__ == "__main__":
    main()
