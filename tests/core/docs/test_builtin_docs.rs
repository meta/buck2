/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use buck2_e2e::golden::golden_dir;
use buck2_e2e::prelude::*;

/// The `.md` files under `dir`, at any depth.
fn md_files(dir: &Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            md_files(&path, found)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            found.push(path);
        }
    }
    Ok(())
}

/// What of a page is pinned: its headings, and all of the sections that the docs generator
/// writes about declared types. The rest of a page is prose.
fn pinned(text: &str) -> String {
    const GENERATED: [&str; 3] = ["## Implements", "## Implemented by", "## Definition"];
    let mut pinned = Vec::new();
    let mut in_generated = false;
    for line in text.lines() {
        if line.starts_with("# ") || line.starts_with("## ") {
            in_generated = GENERATED.contains(&line);
            pinned.push(line);
        } else if line == "---" {
            in_generated = false;
        } else if in_generated && !line.is_empty() {
            pinned.push(line);
        }
    }
    pinned.join("\n")
}

#[buck_test]
async fn test_builtin_docs_golden(buck: &Buck) -> anyhow::Result<()> {
    let output = buck
        .cwd()
        .parent()
        .expect("the project is in the test's directory")
        .join("output");
    buck.docs(&[
        "starlark-builtins",
        "--output-dir",
        &output.to_string_lossy(),
    ])
    .await?;

    let mut files = Vec::new();
    md_files(&output, &mut files)?;
    let outputs = files
        .into_iter()
        .map(|file| {
            let text = std::fs::read_to_string(&file)?;
            let rel_path = file.strip_prefix(&output)?;
            Ok((rel_path.to_string_lossy().replace('\\', "/"), pinned(&text)))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    golden_dir(&outputs, "buck2-golden-docs");

    // A signature links the name of a declared type as it links any type that has a page.
    let cmd_args = std::fs::read_to_string(output.join("build").join("cmd_args.md"))?;
    assert!(
        cmd_args
            .contains(r#"\*args: <Link to="/docs/api/build/CmdArgsInput">CmdArgsInput</Link>,"#),
        "{cmd_args}"
    );
    Ok(())
}
