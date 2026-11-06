/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use buck2_e2e::prelude::*;

/// The names of everything under `dir`, at any depth, without following symlinks.
fn names_under(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let mut names = vec![entry.file_name().to_string_lossy().into_owned()];
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                names.extend(names_under(&entry.path()));
            }
            names
        })
        .collect()
}

#[buck_test]
async fn test_clean_stale_bxl(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//clean_stale/build.bxl:build_test"]).await?;

    let art = buck.cwd().join("buck-out/v2/art");
    let art_files = names_under(&art);
    assert!(art_files.iter().any(|name| name == "out.json"));

    // Check that artifacts written to art by bxl are not deleted
    buck.clean(&["--stale"]).await?;
    let art_files = names_under(&art);
    assert!(art_files.iter().any(|name| name == "out.json"));

    // Force clean of tracked artifacts, check that art and bxl are both deleted
    buck.kill().await?;
    buck.clean(&["--stale=0s"]).await?;

    let art_files = names_under(&art);
    assert!(!art_files.iter().any(|name| name == "out.json"));

    let art_bxl_files = names_under(&buck.cwd().join("buck-out/v2/art-bxl"));
    assert!(!art_bxl_files.iter().any(|name| name == "foo_out"));
    Ok(())
}
