# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Writes an IntelliJ / Android Studio project from the module list `intellij.bxl` collects.

Usage: gen_intellij.py <modules.json> <output dir>

The output directory gets `.idea/modules.xml`, `.idea/misc.xml` and one `.iml` per module. Every
path in them is absolute, so the project can be opened where it is.
"""

from __future__ import annotations

import json
import os
import re
import sys
from collections.abc import Iterable
from xml.sax.saxutils import quoteattr

PACKAGE = re.compile(r"^\s*package\s+([A-Za-z_][\w.]*)", re.MULTILINE)


def package_of(path: str) -> str:
    """The package a Java or Kotlin file declares, or "" for the default package."""
    with open(path, encoding="utf-8", errors="replace") as f:
        m = PACKAGE.search(f.read(64 * 1024))
    return m.group(1) if m else ""


def source_folders(srcs: Iterable[str]) -> dict[str, str]:
    """Maps each directory holding sources to its package prefix: the package its files declare.

    A directory that matches its package's path (`.../com/example/Foo.kt` in `com.example`) gets
    the prefix too; IntelliJ accepts both forms, and this one needs no guess at the root.
    """
    folders: dict[str, str] = {}
    for src in sorted(srcs):
        if src.endswith((".java", ".kt")):
            folders.setdefault(os.path.dirname(src), package_of(src))
    return folders


def url(path: str) -> str:
    return "file://" + path


def jar_url(path: str) -> str:
    if os.path.isdir(path):
        return url(path)
    return f"jar://{path}!/"


def android_facet_xml(android: dict, module_dir: str) -> list[str]:
    """The Android facet: the manifest and `res` directory, relative to the module."""
    lines = [
        '  <component name="FacetManager">',
        '    <facet type="android" name="Android">',
        "      <configuration>",
    ]
    if android.get("manifest"):
        manifest = "/" + os.path.relpath(android["manifest"], module_dir)
        lines.append(
            f'        <option name="MANIFEST_FILE_RELATIVE_PATH" value={quoteattr(manifest)} />'
        )
    for res in android.get("res", [])[:1]:
        res = "/" + os.path.relpath(res, module_dir)
        lines.append(
            f'        <option name="RES_FOLDER_RELATIVE_PATH" value={quoteattr(res)} />'
        )
    return lines + ["      </configuration>", "    </facet>", "  </component>"]


def module_xml(m: dict) -> str:
    lines = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        '<module type="JAVA_MODULE" version="4">',
    ]
    android = m.get("android")
    if android:
        lines += android_facet_xml(android, m["dir"])
    lines += [
        '  <component name="NewModuleRootManager" inherit-compiler-output="true">',
        "    <exclude-output />",
        f"    <content url={quoteattr(url(m['dir']))}>",
    ]
    test = "true" if m["test"] else "false"
    for folder, prefix in sorted(source_folders(m["srcs"]).items()):
        lines.append(
            f'      <sourceFolder url={quoteattr(url(folder))} isTestSource="{test}" packagePrefix={quoteattr(prefix)} />'
        )
    lines += [
        "    </content>",
        '    <orderEntry type="inheritedJdk" />',
        '    <orderEntry type="sourceFolder" forTests="false" />',
    ]
    for dep in m["deps"]:
        lines.append(f'    <orderEntry type="module" module-name={quoteattr(dep)} />')
    for jar in m["libraries"]:
        lines += [
            '    <orderEntry type="module-library">',
            "      <library>",
            f"        <CLASSES><root url={quoteattr(jar_url(jar))} /></CLASSES>",
            "        <JAVADOC />",
            "        <SOURCES />",
            "      </library>",
            "    </orderEntry>",
        ]
    lines += ["  </component>", "</module>", ""]
    return "\n".join(lines)


def write(path: str, text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def main(argv: list[str]) -> int:
    with open(argv[1], encoding="utf-8") as f:
        project = json.load(f)
    out = os.path.abspath(argv[2])
    entries = []
    for m in project["modules"]:
        iml = os.path.join(out, ".idea", "modules", m["name"] + ".iml")
        write(iml, module_xml(m))
        entries.append(
            f"      <module fileurl={quoteattr(url(iml))} filepath={quoteattr(iml)} />"
        )
    write(
        os.path.join(out, ".idea", "modules.xml"),
        "\n".join(
            [
                '<?xml version="1.0" encoding="UTF-8"?>',
                '<project version="4">',
                '  <component name="ProjectModuleManager">',
                "    <modules>",
                *entries,
                "    </modules>",
                "  </component>",
                "</project>",
                "",
            ]
        ),
    )
    jdk = project.get("java_version") or "17"
    write(
        os.path.join(out, ".idea", "misc.xml"),
        "\n".join(
            [
                '<?xml version="1.0" encoding="UTF-8"?>',
                '<project version="4">',
                f'  <component name="ProjectRootManager" version="2" languageLevel="JDK_{jdk}" project-jdk-name={quoteattr(jdk)} project-jdk-type="JavaSDK" />',
                "</project>",
                "",
            ]
        ),
    )
    write(os.path.join(out, ".idea", ".name"), project.get("name") or "buck2")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
