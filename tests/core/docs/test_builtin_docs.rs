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
            let s = text
                .lines()
                .filter(|x| x.starts_with("# ") || x.starts_with("## "))
                .collect::<Vec<_>>()
                .join("\n");

            let rel_path = file.strip_prefix(&output)?;
            Ok((rel_path.to_string_lossy().replace('\\', "/"), s))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    golden_dir(&outputs, "buck2-golden-docs");
    Ok(())
}
