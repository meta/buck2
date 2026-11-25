#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

"""
Generate API documentation for the website.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Dict, List, Optional, Tuple


def read_file(path: Path) -> str:
    with open(path, "r") as f:
        return f.read()


def write_file(path: Path, contents: str) -> None:
    with open(path, "w") as f:
        f.write(contents)


def setup_gen_dir(path: Path) -> None:
    shutil.rmtree(path, ignore_errors=True)
    path.mkdir(parents=True, exist_ok=True)
    (path / "README.txt").write_text(
        """
This directory contains generated files.

Re-generate by running `fbcode/buck2/website/gen_docs.py`.
"""
    )


def buck_command(args: argparse.Namespace) -> str:
    if args.buck2:
        buck = args.buck2
    elif args.prod:
        buck = "buck2"
    elif args.cargo:
        buck = "cargo run --bin=buck2 --"
    else:
        buck = "./buck2.py"
    return buck + " --client-metadata id=buck2_gen_docs"


def rule_summary(page: str) -> str:
    """
    The first sentence of the description of a rule page, for a table cell.
    """
    lines = page.splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith("# ")) + 1
    paragraph: List[str] = []
    for line in lines[start:]:
        if line.startswith("#"):
            break
        if line.strip():
            paragraph.append(line.strip())
        elif paragraph:
            break
    text = " ".join(paragraph)
    # The first period that ends a sentence, and not "e.g." or "i.e.".
    end = re.search(r"(?<!e\.g)(?<!i\.e)\.(?=\s|$)", text)
    if end:
        text = text[: end.end()]
    return text.replace("|", "\\|")


def rules_table(
    rules: List[Tuple[str, str, str]], categories: Optional[Dict[str, str]]
) -> str:
    """
    A table of rules, each given as its name, the directory of its category and
    its summary.

    With `categories`, which maps a directory to the name of its category, the
    table is the one of all rules: it has a column for the category, and its
    links are relative to the directory that holds the category directories.
    Without, the table is the one of a single category, and its links are
    relative to the directory of that category.
    """
    header = "| Rule | Category | Description |\n|---|---|---|\n"
    if categories is None:
        header = "| Rule | Description |\n|---|---|\n"
    rows = []
    for name, directory, summary in sorted(rules):
        if categories is None:
            rows.append(f"| [`{name}`]({name}.md) | {summary} |")
        else:
            link = f"[`{name}`]({directory}/{name}.md)"
            rows.append(f"| {link} | {categories[directory]} | {summary} |")
    return header + "\n".join(rows) + "\n"


def generate_prelude_rules_docs(buck: str) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        base_dir = Path("docs") / "prelude" / "rules"
        setup_gen_dir(base_dir)
        # Actually generate the docs
        print("Running Buck...")
        subprocess.run(
            buck
            + " docs starlark --format=markdown_files --output-dir="
            + tmp
            + " prelude//docs:rules.bzl",
            shell=True,
            check=True,
        )

        # Copy the files under Path(tmp) / prelude / docs / rules to base_dir
        folder = Path(tmp) / "prelude" / "docs" / "rules.bzl"
        # The rules in each directory, as (name, directory, summary).
        rules: Dict[str, List[Tuple[str, str, str]]] = {}
        # The category of each directory. Docusaurus does not publish a
        # directory whose name starts with a dot, so the `.NET` category is
        # written to the directory `NET` and labelled `.NET` in the sidebar.
        categories: Dict[str, str] = {}
        for orig in folder.rglob("*.md"):
            path = orig.relative_to(folder)
            if len(path.parts) == 1:
                # The index of all rules, which is written below.
                continue
            if len(path.parts) != 2:
                raise Exception(f"Expected <category>/<page>.md, got `{path}`")
            category = path.parts[0]
            directory = category.lstrip(".")
            if categories.setdefault(directory, category) != category:
                raise Exception(
                    f"`{category}` and `{categories[directory]}` are both written to `{directory}`"
                )
            dest = base_dir / directory / path.name
            dest.parent.mkdir(parents=True, exist_ok=True)
            if path.name == "index.md":
                continue
            page = read_file(orig)
            write_file(dest, page)
            rules.setdefault(directory, []).append(
                (path.stem, directory, rule_summary(page))
            )

        for directory, category in categories.items():
            write_file(
                base_dir / directory / "index.md",
                f"# {category}\n\n" + rules_table(rules.get(directory, []), None),
            )
            if directory != category:
                write_file(
                    base_dir / directory / "_category_.json",
                    json.dumps({"label": category}) + "\n",
                )

        index_file_content = (
            "# Rules\n\nThe rules of the prelude that have a reference page. A build "
            "file can use\nthem without a `load()`.\n\n"
            + rules_table([r for rs in rules.values() for r in rs], categories)
        )

        os.makedirs(base_dir, exist_ok=True)
        write_file(base_dir / "index.md", index_file_content)


def generate_api_docs(buck: str) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        base_dir = Path("docs") / "api"
        setup_gen_dir(base_dir)
        subprocess.run(
            buck + " docs starlark-builtins --output-dir " + tmp,
            shell=True,
            check=True,
        )

        for orig in Path(tmp).rglob("*.md"):
            path = orig.relative_to(tmp)
            dest = base_dir.joinpath(path)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(orig, dest)


def generate_bxl_utils_api_docs(buck: str) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        base_dir = Path("docs") / "api" / "bxl_utils"
        setup_gen_dir(base_dir)
        bxl_utils_foler = Path("prelude") / "bxl"
        bxl_utils_file_prefix = "prelude//bxl"

        os.makedirs(base_dir, exist_ok=True)

        for file in bxl_utils_foler.rglob("*.bxl"):
            relative_file = file.relative_to(bxl_utils_foler)
            relative_folder_name = (
                "" if file.parent == bxl_utils_foler else str(relative_file.parent)
            )
            buck_bxl_file_name = (
                bxl_utils_file_prefix + relative_folder_name + ":" + relative_file.name
            )

            subprocess.run(
                buck
                + " docs starlark --format=markdown_files --output-dir="
                + tmp
                + " "
                + buck_bxl_file_name,
                shell=True,
                check=True,
            )

            content = read_file(
                Path(tmp) / str(bxl_utils_foler) / (str(relative_file) + ".md")
            )

            # Remove the first line of the file, which is the file path of the bxl file
            content = "\n".join(content.splitlines()[1:])

            prefix = f"""# {file.stem} \n\nThe following functions are defined in the bxl file: `{buck_bxl_file_name}`. \
            You can import them in your bxl file by using `load("@{buck_bxl_file_name}", "function_name")`\n\n"""

            dest = base_dir / (str(relative_file.with_suffix(".md")))

            write_file(dest, prefix + content)


def parse_subcommands(output: str) -> List[str]:
    res = []
    seen_subcommands = False
    for x in output.splitlines():
        if x == "Commands:":
            seen_subcommands = True
        if seen_subcommands and x.startswith("  ") and len(x) > 2 and x[2].isalpha():
            sub = x.strip().split()[0]
            if sub != "help":
                if sub != "cleanall":  # FIXME(scottcao): Not fully implemented yet
                    res.append(sub)
    return res


def generate_help_docs_subcommand(buck: str, args: List[str]) -> str:
    cmd = buck + " docs markdown-help-doc " + " ".join(args)
    print("Running " + cmd + " ...")
    res = subprocess.run(cmd, shell=True, check=True, stdout=subprocess.PIPE)
    return res.stdout.decode()


def generate_subcommand_short_help(buck: str, args: List[str]) -> str:
    cmd = buck + " " + " ".join(args) + " -h"
    print("Running " + cmd + " ...")
    res = subprocess.run(cmd, shell=True, check=True, stdout=subprocess.PIPE)
    output = res.stdout.decode()
    # get the first line which is the short help
    return output.splitlines()[0]


def generate_help_docs_index_page(buck: str, subcommands: List[str]) -> str:
    # Table header
    titile = """\
---
id: index
title: buck2 commands
---
"""
    common_options_section = """\

For common options available across multiple commands, see [Common Options](./common-options).

"""
    lines = [
        "| Command       | Description                  |",
        "|---------------|------------------------------|",
    ]
    for sub in subcommands:
        full_cmd = f"`buck2 {sub}`"
        cmd_with_link = f"[{full_cmd}](./{sub})"
        short_help = generate_subcommand_short_help(buck, [sub])
        # Escape any pipe characters in the help text
        safe_help = short_help.replace("|", "\\|")
        lines.append(f"| {cmd_with_link} | {safe_help} |")

    return titile + common_options_section + "\n".join(lines)


def generate_help_docs(buck: str) -> None:
    base_dir = Path("docs") / "users" / "commands"
    setup_gen_dir(base_dir)

    cmd = buck + " --help"
    print("Running " + cmd + " ...")
    res = subprocess.run(cmd, shell=True, check=True, capture_output=True)
    subcommands = parse_subcommands(res.stdout.decode())
    # Use addtional common-options subcommand to generate the common options page
    for sub in subcommands + ["common-options"]:
        output = generate_help_docs_subcommand(buck, [sub])
        write_file(
            base_dir / (sub + ".generated.md"),
            "---\nid: " + sub + "\ntitle: " + sub + "\n---\n\n" + output,
        )

    # No need to generate a row for the "common-options"
    index_page_content = generate_help_docs_index_page(buck, subcommands)
    write_file(base_dir / "index.md", index_page_content)


def generate_query_docs(buck: str) -> None:
    base_dir = Path("docs") / "users" / "query"
    setup_gen_dir(base_dir)

    for x in ["uquery", "cquery", "aquery"]:
        cmd = buck + " docs " + x + " --format=markdown"
        print("Running " + cmd + " ...")
        res = subprocess.run(cmd, shell=True, check=True, capture_output=True)
        write_file(
            base_dir / (x + ".generated.md"),
            "---\n"
            + f"id: {x}\n"
            + f"title: {x.title()} Environment\n"
            + "toc_max_heading_level: 4\n"
            + "---\n"
            + "\nimport useBaseUrl from '@docusaurus/useBaseUrl';"
            + "\nimport { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';\n\n"
            + res.stdout.decode(),
        )


def generate_settings_docs(buck: str) -> None:
    cmd = buck + " docs settings --format=markdown"
    print("Running " + cmd + " ...")
    res = subprocess.run(cmd, shell=True, check=True, stdout=subprocess.PIPE)
    write_file(
        Path("docs") / "concepts" / "buck_settings_reference.generated.md",
        "---\n"
        + "id: buck_settings_reference\n"
        + "title: Buck settings reference\n"
        + "---\n\n"
        + "Every Buck setting, by section. For the files settings are read from and how they "
        + "override each other, see [Buck settings](buck_settings.md).\n\n"
        + res.stdout.decode(),
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--prod",
        action="store_true",
        default=False,
        help="Whether to use the production `buck2` binary",
    )
    parser.add_argument(
        "--cargo",
        action="store_true",
        default=False,
        help="Whether to use a `cargo` built binary.",
    )
    parser.add_argument(
        "--buck2",
        nargs="?",
        help="Whether to use provided binary.",
    )
    args = parser.parse_args()

    # Change to buck2 directory
    buck2_dir = Path(__file__).absolute().parent.parent
    os.chdir(str(buck2_dir))

    # Clear the docs folder first so that if we change the names of any
    # objects, we'll remove old docs
    for x in Path("docs").rglob("*.generated.md"):
        os.remove(x)

    buck = buck_command(args)
    generate_prelude_rules_docs(buck)
    generate_api_docs(buck)
    generate_bxl_utils_api_docs(buck)
    generate_help_docs(buck)
    generate_query_docs(buck)
    generate_settings_docs(buck)


if __name__ == "__main__":
    main()
