# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import hashlib
import tempfile
import unittest
from pathlib import Path

from buck2.tools.maven_to_buck import maven_to_buck as m

REPO = "https://repo.example"


def pom(group: str, artifact: str, version: str, body: str = "", packaging: str = "jar") -> str:
    return (
        '<project xmlns="http://maven.apache.org/POM/4.0.0">'
        f"<groupId>{group}</groupId><artifactId>{artifact}</artifactId><version>{version}</version>"
        f"<packaging>{packaging}</packaging>{body}</project>"
    )


def dep(group: str, artifact: str, version: str = "", extra: str = "") -> str:
    v = f"<version>{version}</version>" if version else ""
    return f"<dependency><groupId>{group}</groupId><artifactId>{artifact}</artifactId>{v}{extra}</dependency>"


def deps(*items: str) -> str:
    return "<dependencies>" + "".join(items) + "</dependencies>"


class FakeRepo:
    """A Maven repository in a dict: POMs and artifacts, with `.sha1` files for the artifacts."""

    def __init__(self) -> None:
        self.files: dict[str, bytes] = {}

    def add(self, group: str, artifact: str, version: str, pom_xml: str, ext: str = "jar", sources: bool = False) -> None:
        base = f"{REPO}/{group.replace('.', '/')}/{artifact}/{version}/{artifact}-{version}"
        self.files[base + ".pom"] = pom_xml.encode()
        if ext:
            data = f"{artifact}-{version}.{ext}".encode()
            self.files[f"{base}.{ext}"] = data
            self.files[f"{base}.{ext}.sha1"] = hashlib.sha1(data).hexdigest().encode() + b"  file\n"
        if sources:
            self.files[f"{base}-sources.jar.sha1"] = b"0" * 40

    def fetch(self, url: str) -> bytes | None:
        return self.files.get(url)

    def resolve(self, *roots: str) -> dict[m.GA, m.Resolved]:
        repos = m.Repos([REPO], self.fetch)
        return m.resolve([coords(r) for r in roots], m.PomReader(repos))


def coords(s: str) -> tuple[str, str, str]:
    g, a, v = s.split(":")
    return (g, a, v)


def versions(graph: dict[m.GA, m.Resolved]) -> dict[str, str]:
    return {f"{g}:{a}": r.version for (g, a), r in graph.items()}


class VersionTest(unittest.TestCase):
    def test_compare(self) -> None:
        ordered = ["1.0-alpha01", "1.0-beta02", "1.0-rc01", "1.0", "1.0.1", "1.9", "1.10", "2.0"]
        for lo, hi in zip(ordered, ordered[1:]):
            self.assertEqual(m.compare_versions(lo, hi), -1, f"{lo} < {hi}")
            self.assertEqual(m.compare_versions(hi, lo), 1, f"{hi} > {lo}")
        self.assertEqual(m.compare_versions("1.0", "1.0.0"), 0)

    def test_in_range(self) -> None:
        self.assertTrue(m.in_range("1.5", "[1.0,2.0)"))
        self.assertTrue(m.in_range("1.0", "[1.0,2.0)"))
        self.assertFalse(m.in_range("2.0", "[1.0,2.0)"))
        self.assertFalse(m.in_range("1.0", "(1.0,)"))
        self.assertTrue(m.in_range("3.0", "[1.0,)"))


class ResolveTest(unittest.TestCase):
    def test_highest_version_wins(self) -> None:
        r = FakeRepo()
        r.add("g", "app", "1", pom("g", "app", "1", deps(dep("g", "a", "1"), dep("g", "b", "1"))))
        r.add("g", "a", "1", pom("g", "a", "1", deps(dep("g", "c", "1.0"))))
        r.add("g", "b", "1", pom("g", "b", "1", deps(dep("g", "c", "2.0"))))
        r.add("g", "c", "1.0", pom("g", "c", "1.0", deps(dep("g", "old-only", "1"))))
        r.add("g", "c", "2.0", pom("g", "c", "2.0"))
        r.add("g", "old-only", "1", pom("g", "old-only", "1"))
        graph = r.resolve("g:app:1")
        self.assertEqual(versions(graph)["g:c"], "2.0")
        self.assertNotIn(("g", "old-only"), graph, "only the losing version of c asked for it")

    def test_skips_test_provided_and_optional(self) -> None:
        r = FakeRepo()
        body = deps(
            dep("g", "t", "1", "<scope>test</scope>"),
            dep("g", "p", "1", "<scope>provided</scope>"),
            dep("g", "o", "1", "<optional>true</optional>"),
            dep("g", "rt", "1", "<scope>runtime</scope>"),
        )
        r.add("g", "app", "1", pom("g", "app", "1", body))
        r.add("g", "rt", "1", pom("g", "rt", "1"))
        graph = r.resolve("g:app:1")
        self.assertEqual(sorted(versions(graph)), ["g:app", "g:rt"])
        self.assertEqual(graph[("g", "app")].runtime_deps, [("g", "rt")])

    def test_exclusions(self) -> None:
        r = FakeRepo()
        exclusion = "<exclusions><exclusion><groupId>g</groupId><artifactId>c</artifactId></exclusion></exclusions>"
        r.add("g", "app", "1", pom("g", "app", "1", deps(dep("g", "a", "1", exclusion))))
        r.add("g", "a", "1", pom("g", "a", "1", deps(dep("g", "c", "1"))))
        graph = r.resolve("g:app:1")
        self.assertNotIn(("g", "c"), graph)

    def test_parent_properties_and_managed_versions(self) -> None:
        r = FakeRepo()
        parent_body = (
            "<properties><c.version>3.1</c.version></properties>"
            "<dependencyManagement>" + deps(dep("g", "c", "${c.version}")) + "</dependencyManagement>"
        )
        r.add("g", "parent", "7", pom("g", "parent", "7", parent_body, packaging="pom"), ext="")
        child = (
            '<project><parent><groupId>g</groupId><artifactId>parent</artifactId><version>7</version></parent>'
            "<artifactId>app</artifactId>" + deps(dep("g", "c"), dep("${project.groupId}", "d", "${project.version}")) + "</project>"
        )
        r.add("g", "app", "7", child)
        r.add("g", "c", "3.1", pom("g", "c", "3.1"))
        r.add("g", "d", "7", pom("g", "d", "7"))
        self.assertEqual(versions(r.resolve("g:app:7")), {"g:app": "7", "g:c": "3.1", "g:d": "7"})

    def test_bom_import(self) -> None:
        r = FakeRepo()
        r.add("g", "bom", "1", pom("g", "bom", "1", "<dependencyManagement>" + deps(dep("g", "c", "4")) + "</dependencyManagement>", "pom"), ext="")
        body = "<dependencyManagement>" + deps(dep("g", "bom", "1", "<type>pom</type><scope>import</scope>")) + "</dependencyManagement>"
        r.add("g", "app", "1", pom("g", "app", "1", body + deps(dep("g", "c"))))
        r.add("g", "c", "4", pom("g", "c", "4"))
        self.assertEqual(versions(r.resolve("g:app:1"))["g:c"], "4")

    def test_constraints_raise_versions_already_in_the_graph(self) -> None:
        r = FakeRepo()
        constraint = "<dependencyManagement>" + deps(dep("g", "ktx", "1.4")) + "</dependencyManagement>"
        r.add("g", "app", "1", pom("g", "app", "1", deps(dep("g", "lib", "1.4"), dep("g", "ktx", "1.2"))))
        r.add("g", "lib", "1.4", pom("g", "lib", "1.4", constraint))
        r.add("g", "ktx", "1.2", pom("g", "ktx", "1.2"))
        r.add("g", "ktx", "1.4", pom("g", "ktx", "1.4"))
        graph = r.resolve("g:app:1")
        self.assertEqual(versions(graph)["g:ktx"], "1.4", "lib 1.4's constraint should raise ktx")

    def test_constraints_do_not_add_artifacts(self) -> None:
        r = FakeRepo()
        constraint = "<dependencyManagement>" + deps(dep("g", "other", "9")) + "</dependencyManagement>"
        r.add("g", "app", "1", pom("g", "app", "1", constraint))
        self.assertEqual(list(versions(r.resolve("g:app:1"))), ["g:app"])

    def test_version_range(self) -> None:
        r = FakeRepo()
        r.add("g", "app", "1", pom("g", "app", "1", deps(dep("g", "c", "[1.0,2.0)"))))
        r.files[f"{REPO}/g/c/maven-metadata.xml"] = b"<metadata><versioning><versions><version>1.0</version><version>1.5</version><version>2.0</version></versions></versioning></metadata>"
        r.add("g", "c", "1.5", pom("g", "c", "1.5"))
        self.assertEqual(versions(r.resolve("g:app:1"))["g:c"], "1.5")


class RenderTest(unittest.TestCase):
    def render(self, r: FakeRepo, root: str) -> str:
        repos = m.Repos([REPO], r.fetch)
        graph = m.resolve([coords(root)], m.PomReader(repos))
        g, a, _ = root.split(":")
        return m.render(graph[(g, a)], repos, "//libs", "deps.txt")

    def test_aar_with_sources_and_deps(self) -> None:
        r = FakeRepo()
        r.add("com.x", "ui", "2", pom("com.x", "ui", "2", deps(dep("com.x", "core", "2")), "aar"), ext="aar", sources=True)
        r.add("com.x", "core", "2", pom("com.x", "core", "2"))
        text = self.render(r, "com.x:ui:2")
        self.assertTrue(text.startswith("# @generated by buck2/tools/maven_to_buck. Edit deps.txt"))
        self.assertIn(f'url = "mvn:{REPO}:com.x:ui:aar:2"', text)
        self.assertIn(f'sha1 = "{hashlib.sha1(b"ui-2.aar").hexdigest()}"', text)
        self.assertIn(f'url = "mvn:{REPO}:com.x:ui:src:2"', text)
        self.assertIn('android_prebuilt_aar(\n    name = "ui",\n    aar = ":ui-remote",\n    source_jar = ":ui-sources-remote",', text)
        self.assertIn('"//libs/com/x/core:core",', text)

    def test_jar_without_sources(self) -> None:
        r = FakeRepo()
        r.add("g", "c", "1", pom("g", "c", "1"))
        text = self.render(r, "g:c:1")
        self.assertIn('prebuilt_jar(\n    name = "c",\n    binary_jar = ":c-remote",\n    visibility', text)
        self.assertNotIn("sources", text)
        self.assertNotIn("deps", text.split("prebuilt_jar(")[1])

    def test_pom_only_artifact_exports_its_deps(self) -> None:
        r = FakeRepo()
        r.add("g", "kmp", "1", pom("g", "kmp", "1", deps(dep("g", "kmp-jvm", "1")), "pom"), ext="")
        r.add("g", "kmp-jvm", "1", pom("g", "kmp-jvm", "1"))
        text = self.render(r, "g:kmp:1")
        self.assertIn('java_library(\n    name = "kmp",', text)
        self.assertIn('exported_deps = [\n        "//libs/g/kmp-jvm:kmp-jvm",', text)


class FilesTest(unittest.TestCase):
    def test_parse_deps_file(self) -> None:
        text = "# comment\n\ng:a:1  # trailing\ng:b:aar:2\n"
        self.assertEqual(m.parse_deps_file(text), [("g", "a", "1"), ("g", "b", "2")])
        with self.assertRaises(SystemExit):
            m.parse_deps_file("g:a\n")

    def test_write_and_prune(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            out = Path(d)
            stale = out / "g" / "old" / "BUCK"
            stale.parent.mkdir(parents=True)
            stale.write_text(m.HEADER.format(deps="x"))
            mine = out / "hand" / "BUCK"
            mine.parent.mkdir(parents=True)
            mine.write_text("# written by hand\n")
            self.assertEqual(m.write_buck_files({("g", "new"): "x\n"}, out, prune=False), (1, 1))
            self.assertTrue(stale.exists())
            self.assertEqual(m.write_buck_files({("g", "new"): "x\n"}, out, prune=True), (1, 1))
            self.assertFalse(stale.exists())
            self.assertTrue(mine.exists(), "files the tool did not write are never deleted")
            self.assertEqual((out / "g" / "new" / "BUCK").read_text(), "x\n")
