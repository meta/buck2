#!/usr/bin/python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Converts core-for-system-modules.jar from Android SDK release to a Java system image
given to the --system flag for when compiling for Java target 9+.

This entire script is translated from Gradle:
https://cs.android.com/android-studio/platform/tools/base/+/mirror-goog-studio-main:build-system/gradle-core/src/main/java/com/android/build/gradle/internal/dependency/JdkImageTransformDelegate.kt

See also the equivalent feature in Bazel Android build rules:
https://github.com/bazelbuild/rules_android/commit/20c8e5bef957a2334346490934bf3dc54942457c

New comments are marked with "META:"
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path
from typing import List


# META: Translation of ProcessBuilder. Note some tools print error to stdout so print both
# stdout+stderr on failure.
def run_subprocess(args: List[str]) -> bytes:
    try:
        return subprocess.check_output(args, stderr=subprocess.PIPE)
    except subprocess.CalledProcessError as e:
        print(f"Failed to run {args}: exit code={e.returncode}", file=sys.stderr)
        if e.stdout:
            print("stdout:", file=sys.stderr)
            print(e.stdout.decode("utf-8"), file=sys.stderr)
        if e.stderr:
            print("stderr:", file=sys.stderr)
            print(e.stderr.decode("utf-8"), file=sys.stderr)
        sys.exit(1)


class JdkImageTransformDelegate:
    def __init__(
        self,
        system_modules_jar: Path,
        work_dir: Path,
        out_dir: Path,
        jdk_tools: JdkTools,
    ):
        self.system_modules_jar = system_modules_jar
        self.work_dir = work_dir
        self.out_dir = out_dir
        self.jdk_tools = jdk_tools

    def run(self):
        """
        Links a JDK Image from [system_modules_jar] in [out_dir], using [work_dir]
        for the output of any intermediate operations. This is done via the "jlink" tool
        as exposed by [jdk_tools].
        """

        # jlink takes a directory of JMOD files as inputs
        jmod_file = self.make_jmod_file()
        jmod_dir = jmod_file.parent

        self.jdk_tools.link_jmods_into_jdk_image(jmod_dir, "java.base", self.out_dir)

        copy_jrt_fs_jar(self.out_dir, self.jdk_tools)

    def make_module_descriptor_java(self) -> Path:
        """
        Creates a "module-info.java" file describing the contents of system_modules_jar
        """
        module_info_java_content = generate_module_descriptor(
            "java.base", [self.system_modules_jar]
        )
        module_info_java = self.work_dir / "module-info.java"
        with open(module_info_java, "w") as f:
            f.write(module_info_java_content)

        return module_info_java

    def make_module_info_class(self) -> Path:
        """
        Creates and compiles a "module-info.class" file describing the contents of system_modules_jar
        """
        module_info_java = self.make_module_descriptor_java()
        self.jdk_tools.compile_module_descriptor(
            module_info_java, self.system_modules_jar, self.work_dir
        )

        module_info_class = self.work_dir / "module-info.class"
        assert module_info_class.exists(), (
            f"Expected compiled module descriptor file to be created at {module_info_class}"
        )

        return module_info_class

    def make_module_jar(self, work_dir: Path) -> Path:
        """
        Creates a Modular Jar from [system_modules_jar] by compiling and adding a module descriptor
        """
        module_info_class = self.make_module_info_class()

        module_jar = work_dir / "module.jar"
        create_jar(
            self.jdk_tools.java,
            self.jdk_tools.jar_builder,
            module_info_class,
            [self.system_modules_jar],
            module_jar,
        )

        return module_jar

    def make_jmod_file(self) -> Path:
        module_name = "java.base"
        jlink_version = self.jdk_tools.jlink_version()
        jmod_dir = self.work_dir / "jmod"
        os.mkdir(jmod_dir)
        jmod_file = jmod_dir / f"{module_name}.jmod"

        module_jar = self.make_module_jar(self.work_dir)

        self.jdk_tools.create_jmod_from_modular_jar(
            jmod_file, jlink_version, module_jar
        )

        return jmod_file


class JdkTools:
    def __init__(
        self,
        java: Path,
        javac: Path,
        jlink: Path,
        jmod: Path,
        jrt_fs_jar: Path,
        jar_builder: Path,
    ):
        self.java = java
        self.javac = javac
        self.jlink = jlink
        self.jmod = jmod
        self.jrt_fs_location = jrt_fs_jar
        self.jar_builder = jar_builder

    def jlink_version(self):
        return run_subprocess([self.jlink, "--version"]).decode("utf-8").strip()

    def compile_module_descriptor(
        self, module_info_java: Path, system_modules_jar: Path, out_dir: Path
    ):
        classpath_arg_value = str(system_modules_jar)

        run_subprocess(
            [
                self.javac,
                "--system=none",
                f"--patch-module=java.base={classpath_arg_value}",
                "-d",
                out_dir,
                module_info_java,
            ]
        )

    def create_jmod_from_modular_jar(
        self, jmod_file: Path, jlink_version: str, module_jar: Path
    ):
        run_subprocess(
            [
                self.jmod,
                "create",
                "--module-version",
                jlink_version,
                # Use LINUX-OTHER to be compatible with JDK 21+ b/294137077
                "--target-platform",
                "LINUX-OTHER",
                "--class-path",
                module_jar,
                jmod_file,
            ]
        )

    def link_jmods_into_jdk_image(
        self, jmod_dir: Path, module_name: str, out_dir: Path
    ):
        run_subprocess(
            [
                self.jlink,
                "--module-path",
                jmod_dir,
                "--add-modules",
                module_name,
                "--output",
                out_dir,
                "--disable-plugin",
                "system-modules",
            ]
        )


def generate_module_descriptor(moduleName: str, jars: List[Path]) -> str:
    string_builder = ""
    string_builder += f"module {moduleName} {{\n"
    package_name_regex = r"(.*)/[^/]*.class"
    for jar in jars:
        with zipfile.ZipFile(jar) as jar_zip:
            entry_names = [entry.filename for entry in jar_zip.infolist()]
        package_names = set()
        for entry in entry_names:
            match = re.match(package_name_regex, entry)
            if match:
                package = match.group(1).replace("/", ".")
                package_names.add(package)
        for package in sorted(package_names):
            string_builder += f"    exports {package};\n"
    string_builder += "}\n"
    return string_builder


def create_jar(
    java: Path,
    jar_builder: Path,
    module_info_class: Path,
    in_jars: List[Path],
    output_jar: Path,
):
    # META: Our JarBuilder is similar to JarFlinger
    run_subprocess(
        [
            java,
            "-jar",
            jar_builder,
            "--class-files",
            f"{module_info_class.name}:{module_info_class}",
            # META: Our jar builder only supports one append-jar
            "--append-jar",
            in_jars[0],
            "--output",
            output_jar,
        ]
    )


def copy_jrt_fs_jar(out_dir: Path, jdk_tools: JdkTools):
    source = jdk_tools.jrt_fs_location

    copied_libs_dir = out_dir / "lib"
    os.makedirs(copied_libs_dir, exist_ok=True)
    destination = copied_libs_dir / source.name

    # META: Skip manifest.mf filtering since it is not applicable to us
    shutil.copy(source, destination)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--core-for-system-modules-jar", required=True)
    parser.add_argument("--java", required=True)
    parser.add_argument("--javac", required=True)
    parser.add_argument("--jlink", required=True)
    parser.add_argument("--jmod", required=True)
    parser.add_argument("--jrt-fs-jar", required=True)
    parser.add_argument("--jar-builder", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    jdk_tools = JdkTools(
        args.java,
        Path(args.javac),
        Path(args.jlink),
        Path(args.jmod),
        Path(args.jrt_fs_jar),
        Path(args.jar_builder),
    )

    work_dir = tempfile.mkdtemp()
    JdkImageTransformDelegate(
        args.core_for_system_modules_jar,
        Path(work_dir),
        Path(args.output),
        jdk_tools,
    ).run()

    return 0


if __name__ == "__main__":
    sys.exit(main())
