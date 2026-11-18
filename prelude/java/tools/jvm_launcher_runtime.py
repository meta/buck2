# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Links a JDK runtime image that contains a launcher jar as a named module.

A JVM started with `-m <module>/<main class>` from that image has an empty
application classpath, so a Java AOT cache trained with it does not depend on
the path or mtime of any file outside the image.
"""

import argparse
import os
import shutil
import subprocess
import tempfile
import zipfile

# jdk.jlink cannot be linked from a runtime image without packaged modules
# (JEP 493), and jdk.jpackage requires it.
_EXCLUDED_JDK_MODULES = {"jdk.jlink", "jdk.jpackage"}


def _run(cmd: list[str]) -> str:
    # Both streams are captured and shown only on failure: jlink warns on every
    # successful link ("Using incubator modules: jdk.incubator.vector") and prints
    # its errors to stdout.
    result = subprocess.run(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"{cmd[0]} exited with {result.returncode}:\n{result.stdout}{result.stderr}"
        )
    return result.stdout


def _packages(jar: str) -> list[str]:
    packages = set()
    with zipfile.ZipFile(jar) as zf:
        for name in zf.namelist():
            if (
                name.endswith(".class")
                and "/" in name
                and not name.startswith("META-INF/")
            ):
                packages.add(name.rsplit("/", 1)[0].replace("/", "."))
    return sorted(packages)


def _jdk_modules(java: str) -> list[str]:
    modules = []
    for line in _run([java, "--list-modules"]).splitlines():
        module = line.strip().split("@", 1)[0]
        if module and module not in _EXCLUDED_JDK_MODULES:
            modules.append(module)
    return modules


def _required_modules(java: str, jar: str) -> list[str]:
    deps = _run(
        [
            java,
            "-m",
            "jdk.jdeps/com.sun.tools.jdeps.Main",
            "--ignore-missing-deps",
            "--print-module-deps",
            jar,
        ]
    ).strip()
    return [m for m in deps.split(",") if m and m != "java.base"]


def _modular_jar(java: str, javac: str, jar: str, module: str, tmp: str) -> str:
    source = os.path.join(tmp, "src", "module-info.java")
    os.makedirs(os.path.dirname(source))
    lines = [f"open module {module} {{"]
    lines += [f"  requires transitive {m};" for m in _required_modules(java, jar)]
    lines += [f"  exports {p};" for p in _packages(jar)]
    lines.append("}")
    with open(source, "w") as f:
        f.write("\n".join(lines) + "\n")

    classes = os.path.join(tmp, "classes")
    _run(
        [
            javac,
            "-nowarn",
            "-d",
            classes,
            "--patch-module",
            f"{module}={jar}",
            source,
        ]
    )

    out = os.path.join(tmp, f"{module}.jar")
    shutil.copyfile(jar, out)
    with zipfile.ZipFile(out, "a") as zf:
        info = zipfile.ZipInfo("module-info.class", date_time=(1985, 2, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        with open(os.path.join(classes, "module-info.class"), "rb") as f:
            zf.writestr(info, f.read())
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", required=True)
    parser.add_argument("--javac", required=True)
    parser.add_argument("--jlink", required=True)
    parser.add_argument("--launcher-jar", required=True)
    parser.add_argument("--module", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    with tempfile.TemporaryDirectory() as tmp:
        jar = _modular_jar(args.java, args.javac, args.launcher_jar, args.module, tmp)
        _run(
            [
                args.jlink,
                "--module-path",
                jar,
                "--add-modules",
                ",".join([args.module] + _jdk_modules(args.java)),
                "--no-header-files",
                "--no-man-pages",
                # jlink drops the JDK's default CDS archive; regenerate it so the
                # runtime starts as fast as the JDK it was linked from when no
                # AOT cache is given.
                "--generate-cds-archive",
                "--output",
                args.output,
            ]
        )


if __name__ == "__main__":
    main()
